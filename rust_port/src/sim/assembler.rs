//! Native Rust port of `avr8js/src/utils/assembler.ts`.
//!
//! The assembler is a two-pass, byte-addressed dialect with its own grammar and
//! diagnostics. Behavior here intentionally mirrors the upstream TypeScript,
//! including its lenient integer parsing and unusual directive handling, because
//! the converted scenarios assert the exact bytes, line tables and error strings.
use std::collections::BTreeMap;

/// One encoded instruction: either raw hex words or a forward-reference deferred
/// to pass two because a label was not yet known.
#[derive(Clone, Debug)]
enum Encoded {
    /// A single 16-bit word, formatted as 4 hex digits in upstream's memory order.
    Word(String),
    /// One or more 16-bit words (currently at most two).
    Words(Vec<String>),
    /// Branch/call/jump whose target label was missing in pass one.
    Deferred {
        op: String,
        a: Option<String>,
        b: Option<String>,
        byte_loc: usize,
        words: usize,
    },
}

impl Encoded {
    fn size(&self) -> usize {
        match self {
            Encoded::Word(_) => 2,
            Encoded::Words(words) => 2 * words.len(),
            Encoded::Deferred { words, .. } => 2 * words,
        }
    }
}

/// One pass-one line-table entry. `text` is the trimmed source line including any
/// trailing comment, matching upstream's captured `lt.text`.
#[derive(Clone, Debug)]
struct Pass1Line {
    line: usize,
    text: String,
    byte_offset: usize,
    bytes: Encoded,
}

#[derive(Debug)]
pub enum LineBytes {
    Word(String),
    Words(Vec<String>),
}

#[derive(Debug)]
pub struct LineEntry {
    pub line: usize,
    pub text: String,
    pub byte_offset: usize,
    pub bytes: LineBytes,
}

#[derive(Debug)]
pub struct AssembleResult {
    pub bytes: Vec<u8>,
    pub errors: Vec<String>,
    pub lines: Vec<LineEntry>,
    pub labels: BTreeMap<String, usize>,
}

/// Assemble an AVR program using the upstream dialect.
pub fn assemble(input: &str) -> AssembleResult {
    let (lines, labels, errors) = pass_one(input);
    if !errors.is_empty() {
        return AssembleResult {
            bytes: Vec::new(),
            errors,
            lines: Vec::new(),
            labels: BTreeMap::new(),
        };
    }
    pass_two(lines, labels)
}

/// JavaScript `parseInt`, including `0x` prefix detection and prefix-only parsing.
fn js_parse_int(value: &str) -> f64 {
    let trimmed = value.trim_start();
    let (sign, rest) = match trimmed.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    let (radix, digits) = match rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X")) {
        Some(hex) => (16u32, hex),
        None => (10u32, rest),
    };
    let mut total = 0.0;
    let mut found = false;
    for c in digits.chars() {
        match c.to_digit(radix) {
            Some(digit) => {
                total = total * f64::from(radix) + f64::from(digit);
                found = true;
            }
            None => break,
        }
    }
    if found {
        sign * total
    } else {
        f64::NAN
    }
}

/// Render a number the way JavaScript stringifies it for diagnostics.
fn js_number(value: f64) -> String {
    if value.fract() == 0.0 && value.is_finite() {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// JavaScript `Number(r).toString(16)` padded on the left to `len` characters.
/// Note upstream does not truncate values longer than `len`.
fn zero_pad(value: f64, len: usize) -> String {
    let text = if value < 0.0 {
        format!("-{:x}", (-value) as i64)
    } else if value.fract() == 0.0 && value.is_finite() {
        format!("{:x}", value as i64)
    } else {
        format!("{value}")
    };
    if text.len() >= len {
        text
    } else {
        format!("{}{}", "0".repeat(len - text.len()), text)
    }
}

/// Two's-complement fit into `bits`, mirroring upstream's `fitTwoC`.
fn fit_two_c(value: f64, bits: u32) -> Result<u32, String> {
    if bits < 2 {
        return Err("Need at least 2 bits to be signed.".into());
    }
    if bits > 16 {
        return Err("fitTwoC only works on 16bit numbers for now.".into());
    }
    if value.abs() > 2f64.powi(bits as i32 - 1) {
        return Err(format!(
            "Not enough bits for number. ({}, {})",
            js_number(value),
            bits
        ));
    }
    let mut adjusted = value;
    if adjusted < 0.0 {
        adjusted = 0xffff as f64 + adjusted + 1.0;
    }
    let mask = 0xffffu32 >> (16 - bits);
    Ok((adjusted as i64 as u32) & mask)
}

/// Constants accepted by the assembler: any JS-parsable integer inside a range.
fn const_value(value: &str, min: f64, max: f64) -> Result<f64, String> {
    let parsed = js_parse_int(value);
    if parsed.is_nan() {
        return Err("constant is not a number.".into());
    }
    if parsed < min || parsed > max {
        return Err(format!(
            "[Ks] out of range: {}<>{}",
            js_number(min),
            js_number(max)
        ));
    }
    Ok(parsed)
}

/// Resolve an operand as an absolute value or label, relative to `offset`.
/// Returns `None` where upstream returns `NaN` for an unknown label.
fn const_or_label(value: &str, labels: &BTreeMap<String, usize>, offset: usize) -> Option<f64> {
    let parsed = js_parse_int(value);
    if !parsed.is_nan() {
        return Some(parsed);
    }
    labels.get(value).map(|label| *label as f64 - offset as f64)
}

/// Destination register index, shifted to the high nibble of the opcode.
fn dest_rindex(register: &str, min: u32, max: u32) -> Result<u32, String> {
    match register.match_digits() {
        Some(index) => {
            if index < min || index > max {
                return Err(format!("Rd out of range: {min}<>{}", max));
            }
            Ok((index & 0x1f) << 4)
        }
        None => Err(format!("Not a register: {register}")),
    }
}

/// Source register index, split across the low nibble and bit 9.
fn src_rindex(register: &str, min: u32, max: u32) -> Result<u32, String> {
    match register.match_digits() {
        Some(index) => {
            if index < min || index > max {
                return Err(format!("Rd out of range: {min}<>{}", max));
            }
            Ok((index & 0xf) | (((index >> 4) & 1) << 9))
        }
        None => Err(format!("Not a register: {register}")),
    }
}

/// Find the first `[Rr]` followed by one or two digits, like upstream's regex.
trait MatchDigits {
    fn match_digits(&self) -> Option<u32>;
}
impl MatchDigits for str {
    fn match_digits(&self) -> Option<u32> {
        let bytes = self.as_bytes();
        for (i, &byte) in bytes.iter().enumerate() {
            if byte == b'r' || byte == b'R' {
                let start = i + 1;
                if start < bytes.len() && bytes[start].is_ascii_digit() {
                    let end = if start + 1 < bytes.len() && bytes[start + 1].is_ascii_digit() {
                        start + 2
                    } else {
                        start + 1
                    };
                    return self[start..end].parse().ok();
                }
            }
        }
        None
    }
}

/// Match the `[Rr](24|26|28|30)` form used by ADIW/SBIW.
fn match_adjust_pair(value: &str) -> Option<u32> {
    let bytes = value.as_bytes();
    for (i, &byte) in bytes.iter().enumerate() {
        if byte == b'r' || byte == b'R' {
            let rest = &value[i + 1..];
            for candidate in [24u32, 26, 28, 30] {
                let text = candidate.to_string();
                if rest.starts_with(&text) {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// Indirect-address opcodes for LD/ST on X/Y/Z with optional pre/post increment.
fn stld_xyz(value: &str) -> Result<u32, String> {
    Ok(match value {
        "X" => 0x900c,
        "X+" => 0x900d,
        "-X" => 0x900e,
        "Y" => 0x8008,
        "Y+" => 0x9009,
        "-Y" => 0x900a,
        "Z" => 0x8000,
        "Z+" => 0x9001,
        "-Z" => 0x9002,
        _ => return Err("Not -?[XYZ]\\+?".into()),
    })
}

/// Indirect-address opcodes for LDD/STD with Y/Z and a displacement.
fn stld_yzq(value: &str) -> Result<u32, String> {
    let bytes = value.as_bytes();
    let mut base = None;
    let mut digits = String::new();
    for (i, &byte) in bytes.iter().enumerate() {
        if byte == b'Y' || byte == b'Z' {
            if let Some(plus) = bytes.get(i + 1) {
                if *plus == b'+' {
                    let rest = &value[i + 2..];
                    if let Some(first) = rest.chars().next() {
                        if first.is_ascii_digit() {
                            base = Some(byte);
                            digits = rest.chars().take_while(char::is_ascii_digit).collect();
                            break;
                        }
                    }
                }
            }
        }
    }
    let base = base.ok_or_else(|| "Invalid arguments".to_string())?;
    let mut result = 0x8000u32;
    if base == b'Y' {
        result |= 0x8;
    }
    let q = digits.parse::<u32>().unwrap_or(0);
    if q > 64 {
        return Err("q is out of range".into());
    }
    result |= ((q & 0x20) << 8) | ((q & 0x18) << 7) | (q & 0x7);
    Ok(result)
}

/// Encode one mnemonic plus operands, or defer when a label is not yet known.
fn encode(
    op: &str,
    a: Option<&str>,
    b: Option<&str>,
    byte_loc: usize,
    labels: &BTreeMap<String, usize>,
) -> Result<Encoded, String> {
    let a = a.unwrap_or("");
    let b = b.unwrap_or("");
    let word = |value: u32| Ok(Encoded::Word(zero_pad(f64::from(value), 4)));
    let pair = |high: u32, low: u32| {
        Ok(Encoded::Words(vec![
            zero_pad(f64::from(high), 4),
            zero_pad(f64::from(low), 4),
        ]))
    };
    match op {
        "ADD" => word(0x0c00 | dest_rindex(a, 0, 31)? | src_rindex(b, 0, 31)?),
        "ADC" => word(0x1c00 | dest_rindex(a, 0, 31)? | src_rindex(b, 0, 31)?),
        "ADIW" | "SBIW" => {
            let base = match match_adjust_pair(a) {
                Some(pair) => pair,
                None => return Err("Rd must be 24, 26, 28, or 30".into()),
            };
            let mut result = if op == "ADIW" { 0x9600 } else { 0x9700 };
            let d = (base - 24) / 2;
            result |= (d & 0x3) << 4;
            let k = const_value(b, 0.0, 63.0)? as u32;
            result |= ((k & 0x30) << 2) | (k & 0x0f);
            word(result)
        }
        "AND" => word(0x2000 | dest_rindex(a, 0, 31)? | src_rindex(b, 0, 31)?),
        "ANDI" | "CPI" | "LDI" | "ORI" | "SBCI" | "SBR" | "SUBI" => {
            let mut result = match op {
                "ANDI" => 0x7000,
                "CPI" => 0x3000,
                "LDI" => 0xe000,
                "ORI" | "SBR" => 0x6000,
                "SBCI" => 0x4000,
                _ => 0x5000,
            };
            result |= dest_rindex(a, 16, 31)? & 0xf0;
            let k = const_value(b, 0.0, 255.0)? as u32;
            result |= ((k & 0xf0) << 4) | (k & 0xf);
            word(result)
        }
        "ASR" => word(0x9405 | dest_rindex(a, 0, 31)?),
        "BCLR" | "BSET" => {
            let mut result = if op == "BCLR" { 0x9488 } else { 0x9408 };
            let s = const_value(a, 0.0, 7.0)? as u32;
            result |= (s & 0x7) << 4;
            word(result)
        }
        "BLD" => word(0xf800 | dest_rindex(a, 0, 31)? | (const_value(b, 0.0, 7.0)? as u32 & 0x7)),
        "BRBC" | "BRBS" => {
            let offset = byte_loc + 2;
            let target = match const_or_label(b, labels, offset) {
                Some(target) => target,
                None => {
                    return Ok(Encoded::Deferred {
                        op: op.into(),
                        a: Some(a.into()),
                        b: Some(b.into()),
                        byte_loc,
                        words: 1,
                    })
                }
            };
            let mut result = if op == "BRBC" { 0xf400 } else { 0xf000 };
            result |= const_value(a, 0.0, 7.0)? as u32;
            let shifted = (target as i64 >> 1) as f64;
            result |= fit_two_c(const_value(&js_number(shifted), -64.0, 63.0)?, 7)? << 3;
            word(result)
        }
        "BRCC" => encode("BRBC", Some("0"), Some(a), byte_loc, labels),
        "BRCS" | "BRLO" => encode("BRBS", Some("0"), Some(a), byte_loc, labels),
        "BREAK" => word(0x9598),
        "BREQ" => encode("BRBS", Some("1"), Some(a), byte_loc, labels),
        "BRGE" | "BRLT" => encode(
            if op == "BRGE" { "BRBC" } else { "BRBS" },
            Some("4"),
            Some(a),
            byte_loc,
            labels,
        ),
        "BRHC" => encode("BRBC", Some("5"), Some(a), byte_loc, labels),
        "BRHS" => encode("BRBS", Some("5"), Some(a), byte_loc, labels),
        "BRID" => encode("BRBC", Some("7"), Some(a), byte_loc, labels),
        "BRIE" => encode("BRBS", Some("7"), Some(a), byte_loc, labels),
        "BRMI" => encode("BRBS", Some("2"), Some(a), byte_loc, labels),
        "BRNE" => encode("BRBC", Some("1"), Some(a), byte_loc, labels),
        "BRPL" => encode("BRBC", Some("2"), Some(a), byte_loc, labels),
        "BRSH" => encode("BRBC", Some("0"), Some(a), byte_loc, labels),
        "BRTC" => encode("BRBC", Some("6"), Some(a), byte_loc, labels),
        "BRTS" => encode("BRBS", Some("6"), Some(a), byte_loc, labels),
        "BRVC" => encode("BRBC", Some("3"), Some(a), byte_loc, labels),
        "BRVS" => encode("BRBS", Some("3"), Some(a), byte_loc, labels),
        "BST" => word(0xfa00 | dest_rindex(a, 0, 31)? | const_value(b, 0.0, 7.0)? as u32),
        "CALL" | "JMP" => {
            let target = match const_or_label(a, labels, 0) {
                Some(target) => target,
                None => {
                    return Ok(Encoded::Deferred {
                        op: op.into(),
                        a: Some(a.into()),
                        b: None,
                        byte_loc,
                        words: 2,
                    })
                }
            };
            let mut result = if op == "CALL" { 0x940e } else { 0x940c };
            let k = const_value(&js_number(target), 0.0, 0x400000 as f64)? as u32 >> 1;
            let low = k & 0xffff;
            let high = (k >> 16) & 0x3f;
            result |= ((high & 0x3e) << 3) | (high & 1);
            pair(result, low)
        }
        "CBI" | "SBI" | "SBIC" | "SBIS" => {
            let mut result = match op {
                "CBI" => 0x9800,
                "SBI" => 0x9a00,
                "SBIC" => 0x9900,
                _ => 0x9b00,
            };
            result |= (const_value(a, 0.0, 31.0)? as u32) << 3;
            result |= const_value(b, 0.0, 7.0)? as u32;
            word(result)
        }
        "CRB" => {
            let k = const_value(b, 0.0, 255.0)? as u32;
            encode(
                "ANDI",
                Some(a),
                Some(&js_number((!k & 0xff) as f64)),
                byte_loc,
                labels,
            )
        }
        "CLC" => word(0x9488),
        "CLH" => word(0x94d8),
        "CLI" => word(0x94f8),
        "CLN" => word(0x94a8),
        "CLR" => encode("EOR", Some(a), Some(a), byte_loc, labels),
        "CLS" => word(0x94c8),
        "CLT" => word(0x94e8),
        "CLV" => word(0x94b8),
        "CLZ" => word(0x9498),
        "COM" => word(0x9400 | dest_rindex(a, 0, 31)?),
        "CP" => word(0x1400 | dest_rindex(a, 0, 31)? | src_rindex(b, 0, 31)?),
        "CPC" => word(0x0400 | dest_rindex(a, 0, 31)? | src_rindex(b, 0, 31)?),
        "CPSE" => word(0x1000 | dest_rindex(a, 0, 31)? | src_rindex(b, 0, 31)?),
        "DEC" => word(0x940a | dest_rindex(a, 0, 31)?),
        "DES" => word(0x940b | (const_value(a, 0.0, 15.0)? as u32) << 4),
        "EICALL" => word(0x9519),
        "EIJMP" => word(0x9419),
        "ELPM" => {
            if a.is_empty() {
                word(0x95d8)
            } else {
                let mut result = 0x9000 | dest_rindex(a, 0, 31)?;
                match b {
                    "Z" => result |= 6,
                    "Z+" => result |= 7,
                    _ => return Err("Bad operand".into()),
                }
                word(result)
            }
        }
        "EOR" => word(0x2400 | dest_rindex(a, 0, 31)? | src_rindex(b, 0, 31)?),
        "FMUL" | "FMULS" | "FMULSU" | "MULSU" => {
            let mut result = match op {
                "FMUL" => 0x0308,
                "FMULS" => 0x0380,
                "FMULSU" => 0x0388,
                _ => 0x0300,
            };
            result |= dest_rindex(a, 16, 23)? & 0x70;
            result |= src_rindex(b, 16, 23)? & 0x7;
            word(result)
        }
        "ICALL" => word(0x9509),
        "IJMP" => word(0x9409),
        "IN" => {
            let mut result = 0xb000 | dest_rindex(a, 0, 31)?;
            let address = const_value(b, 0.0, 63.0)? as u32;
            result |= ((address & 0x30) << 5) | (address & 0x0f);
            word(result)
        }
        "INC" => word(0x9403 | dest_rindex(a, 0, 31)?),
        "LAC" | "LAS" | "LAT" => {
            if a != "Z" {
                return Err("First Operand is not Z".into());
            }
            let base = match op {
                "LAC" => 0x9206,
                "LAS" => 0x9205,
                _ => 0x9207,
            };
            word(base | dest_rindex(b, 0, 31)?)
        }
        "LD" => word(dest_rindex(a, 0, 31)? | stld_xyz(b)?),
        "LDD" => word(dest_rindex(a, 0, 31)? | stld_yzq(b)?),
        "LDS" => {
            let k = const_value(b, 0.0, 65535.0)? as u32;
            pair(0x9000 | dest_rindex(a, 0, 31)?, k)
        }
        "LPM" => {
            if a.is_empty() {
                word(0x95c8)
            } else {
                let mut result = 0x9000 | dest_rindex(a, 0, 31)?;
                match b {
                    "Z" => result |= 4,
                    "Z+" => result |= 5,
                    _ => return Err("Bad operand".into()),
                }
                word(result)
            }
        }
        "LSL" => encode("ADD", Some(a), Some(a), byte_loc, labels),
        "LSR" => word(0x9406 | dest_rindex(a, 0, 31)?),
        "MOV" => word(0x2c00 | dest_rindex(a, 0, 31)? | src_rindex(b, 0, 31)?),
        "MOVW" => word(
            0x0100 | ((dest_rindex(a, 0, 31)? >> 1) & 0xf0) | ((dest_rindex(b, 0, 31)? >> 5) & 0xf),
        ),
        "MUL" => word(0x9c00 | dest_rindex(a, 0, 31)? | src_rindex(b, 0, 31)?),
        "MULS" => word(0x0200 | (dest_rindex(a, 16, 31)? & 0xf0) | (src_rindex(b, 16, 31)? & 0xf)),
        "NEG" => word(0x9401 | dest_rindex(a, 0, 31)?),
        "NOP" => word(0x0000),
        "OR" => word(0x2800 | dest_rindex(a, 0, 31)? | src_rindex(b, 0, 31)?),
        "OUT" => {
            let mut result = 0xb800 | dest_rindex(b, 0, 31)?;
            let address = const_value(a, 0.0, 63.0)? as u32;
            result |= ((address & 0x30) << 5) | (address & 0x0f);
            word(result)
        }
        "POP" => word(0x900f | dest_rindex(a, 0, 31)?),
        "PUSH" => word(0x920f | dest_rindex(a, 0, 31)?),
        "RCALL" | "RJMP" => {
            let offset = byte_loc + 2;
            let target = match const_or_label(a, labels, offset) {
                Some(target) => target,
                None => {
                    return Ok(Encoded::Deferred {
                        op: op.into(),
                        a: Some(a.into()),
                        b: None,
                        byte_loc,
                        words: 1,
                    })
                }
            };
            let base = if op == "RCALL" { 0xd000 } else { 0xc000 };
            let shifted = (target as i64 >> 1) as f64;
            let encoded = fit_two_c(const_value(&js_number(shifted), -2048.0, 2047.0)?, 12)?;
            word(base | encoded)
        }
        "RET" => word(0x9508),
        "RETI" => word(0x9518),
        "ROL" => encode("ADC", Some(a), Some(a), byte_loc, labels),
        "ROR" => word(0x9407 | dest_rindex(a, 0, 31)?),
        "SBC" => word(0x0800 | dest_rindex(a, 0, 31)? | src_rindex(b, 0, 31)?),
        "SBRC" => word(0xfc00 | dest_rindex(a, 0, 31)? | const_value(b, 0.0, 7.0)? as u32),
        "SBRS" => word(0xfe00 | dest_rindex(a, 0, 31)? | const_value(b, 0.0, 7.0)? as u32),
        "SEC" | "SEH" | "SEI" | "SEN" | "SES" | "SET" | "SEV" | "SEZ" => {
            let bit = match op {
                "SEC" => 0,
                "SEZ" => 1,
                "SEN" => 2,
                "SEV" => 3,
                "SES" => 4,
                "SEH" => 5,
                "SET" => 6,
                _ => 7,
            };
            word(0x9408 | (bit << 4))
        }
        "SER" => word(0xef0f | (dest_rindex(a, 16, 31)? & 0xf0)),
        "SLEEP" => word(0x9588),
        "SPM" => {
            if a.is_empty() {
                word(0x95e8)
            } else if a == "Z+" {
                word(0x95f8)
            } else {
                Err("Bad param to SPM".into())
            }
        }
        "ST" => word(0x0200 | dest_rindex(b, 0, 31)? | stld_xyz(a)?),
        "STD" => word(0x0200 | dest_rindex(b, 0, 31)? | stld_yzq(a)?),
        "STS" => {
            let k = const_value(a, 0.0, 65535.0)? as u32;
            pair(0x9200 | dest_rindex(b, 0, 31)?, k)
        }
        "SUB" => word(0x1800 | dest_rindex(a, 0, 31)? | src_rindex(b, 0, 31)?),
        "SWAP" => word(0x9402 | dest_rindex(a, 0, 31)?),
        "TST" => encode("AND", Some(a), Some(a), byte_loc, labels),
        "WDR" => word(0x95a8),
        "XCH" => {
            if a != "Z" {
                return Err("Bad param, not Z".into());
            }
            word(0x9204 | dest_rindex(b, 0, 31)?)
        }
        _ => Err(format!("No such instruction: {op}")),
    }
}

/// Parse the upstream `codeReg` grammar into mnemonic and up to two operands.
fn parse_code(line: &str) -> Option<(String, Option<String>, Option<String>)> {
    let text = line.trim_start();
    let mnemonic_end = text
        .char_indices()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
        .last()
        .map(|(i, c)| i + c.len_utf8());
    let mnemonic_end = mnemonic_end?;
    let mnemonic = &text[..mnemonic_end];
    if mnemonic.is_empty() {
        return None;
    }
    let rest = &text[mnemonic_end..];
    if rest.is_empty() {
        return Some((mnemonic.into(), None, None));
    }
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    // `([^,]+)` is greedy but cannot cross the first comma.
    let comma = rest.find(',');
    let operand2 = match comma {
        Some(index) => &rest[..index],
        None => rest,
    };
    if operand2.is_empty() {
        return None;
    }
    let operand2 = Some(operand2.to_string());
    let operand3 = match comma {
        Some(index) => {
            let after = rest[index + 1..].trim_start();
            let end = after
                .char_indices()
                .take_while(|(_, c)| !c.is_whitespace())
                .last()
                .map(|(i, c)| i + c.len_utf8());
            match end {
                Some(end) => Some(after[..end].to_string()),
                None => return None,
            }
        }
        None => None,
    };
    Some((mnemonic.into(), operand2, operand3))
}

fn pass_one(input: &str) -> (Vec<Pass1Line>, BTreeMap<String, usize>, Vec<String>) {
    let mut labels: BTreeMap<String, usize> = BTreeMap::new();
    let mut errors = Vec::new();
    let mut line_table: Vec<Pass1Line> = Vec::new();
    let mut replacements: BTreeMap<String, String> = BTreeMap::new();
    let mut byte_offset = 0usize;

    for (index, raw) in input.split('\n').enumerate() {
        let mut text = raw.trim().to_string();
        if text.is_empty() {
            continue;
        }
        let original = text.clone();
        if let Some(position) = text.find(['#', ';']) {
            text.truncate(position);
            text = text.trim().to_string();
        }
        if text.is_empty() {
            continue;
        }
        // A leading `\w+:` label records the current byte offset.
        if let Some(colon) = text.find(':') {
            let name = &text[..colon];
            if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                labels.insert(name.into(), byte_offset);
                text = text[colon + 1..].trim().to_string();
            }
        }
        if text.is_empty() {
            continue;
        }
        let parsed = match parse_code(&text) {
            Some(parsed) => parsed,
            None => {
                errors.push(format!("Line {index}: doesn't match as code!"));
                continue;
            }
        };
        let (mnemonic_raw, mut operand2, mut operand3) = parsed;
        let mnemonic = mnemonic_raw.to_uppercase();
        match mnemonic.as_str() {
            "_REPLACE" => {
                if let (Some(key), Some(value)) = (operand2.clone(), operand3.clone()) {
                    replacements.insert(key, value);
                }
                continue;
            }
            "_LOC" => {
                let number = js_parse_int(operand2.as_deref().unwrap_or(""));
                if number.is_nan() {
                    errors.push(format!("Line {index}: Location is not a number."));
                    continue;
                }
                let number = number as i64;
                if number & 1 != 0 {
                    errors.push(format!("Line {index}: Location is odd"));
                    continue;
                }
                byte_offset = number as usize;
                continue;
            }
            "_IW" => {
                let number = js_parse_int(operand2.as_deref().unwrap_or(""));
                if number.is_nan() {
                    errors.push(format!("Line {index}: Immeadiate Word is not a number."));
                    continue;
                }
                byte_offset += 2;
                continue;
            }
            _ => {}
        }
        if let Some(operand) = operand2.as_mut() {
            if let Some(replacement) = replacements.get(operand.as_str()) {
                *operand = replacement.clone();
            }
        }
        if let Some(operand) = operand3.as_mut() {
            if let Some(replacement) = replacements.get(operand.as_str()) {
                *operand = replacement.clone();
            }
        }
        match encode(
            &mnemonic,
            operand2.as_deref(),
            operand3.as_deref(),
            byte_offset,
            &labels,
        ) {
            Ok(encoded) => {
                let size = encoded.size();
                line_table.push(Pass1Line {
                    line: index + 1,
                    text: original,
                    byte_offset,
                    bytes: encoded,
                });
                byte_offset += size;
            }
            Err(error) => errors.push(format!("Line {index}: {error}")),
        }
    }
    (line_table, labels, errors)
}

fn pass_two(lines: Vec<Pass1Line>, labels: BTreeMap<String, usize>) -> AssembleResult {
    let mut errors = Vec::new();
    let mut table: Vec<LineEntry> = Vec::new();
    for entry in &lines {
        let bytes = match &entry.bytes {
            Encoded::Deferred {
                op, a, b, byte_loc, ..
            } => match encode(op, a.as_deref(), b.as_deref(), *byte_loc, &labels) {
                Ok(encoded) => encoded,
                Err(error) => {
                    errors.push(format!("Line: {}: {error}", entry.line));
                    continue;
                }
            },
            other => other.clone(),
        };
        let out = match bytes {
            Encoded::Word(word) => LineBytes::Word(word),
            Encoded::Words(words) => LineBytes::Words(words),
            Encoded::Deferred { .. } => unreachable!(),
        };
        table.push(LineEntry {
            line: entry.line,
            text: entry.text.clone(),
            byte_offset: entry.byte_offset,
            bytes: out,
        });
    }
    // Size from the last line-table entry, exactly like upstream's `elementSize`.
    let byte_size = lines
        .last()
        .map(|entry| entry.byte_offset + entry.bytes.size())
        .unwrap_or(0);
    let mut bytes = vec![0u8; byte_size];
    for entry in &table {
        let mut write = |offset: usize, word: &str| {
            if word.len() < 4 {
                return;
            }
            let low = u8::from_str_radix(&word[0..2], 16).unwrap_or(0);
            let high = u8::from_str_radix(&word[2..4], 16).unwrap_or(0);
            if offset + 1 < bytes.len() {
                bytes[offset] = high;
                bytes[offset + 1] = low;
            } else if offset < bytes.len() {
                bytes[offset] = high;
            }
        };
        match &entry.bytes {
            LineBytes::Word(word) => write(entry.byte_offset, word),
            LineBytes::Words(words) => {
                let mut offset = entry.byte_offset;
                for word in words {
                    write(offset, word);
                    offset += 2;
                }
            }
        }
    }
    AssembleResult {
        bytes,
        errors,
        lines: table,
        labels,
    }
}

/// Convert an assembly result into the dynamic `Value` shape the scenarios expect.
pub fn to_value(result: AssembleResult) -> crate::runtime::Value {
    use crate::runtime::Value;
    let lines: Vec<Value> = result
        .lines
        .into_iter()
        .map(|entry| {
            let bytes = match entry.bytes {
                LineBytes::Word(word) => Value::Text(word),
                LineBytes::Words(words) => {
                    Value::array(words.into_iter().map(Value::Text).collect())
                }
            };
            Value::object(BTreeMap::from([
                ("byteOffset".into(), Value::Number(entry.byte_offset as f64)),
                ("bytes".into(), bytes),
                ("line".into(), Value::Number(entry.line as f64)),
                ("text".into(), Value::Text(entry.text)),
            ]))
        })
        .collect();
    let labels: BTreeMap<String, Value> = result
        .labels
        .into_iter()
        .map(|(name, value)| (name, Value::Number(value as f64)))
        .collect();
    Value::object(BTreeMap::from([
        ("bytes".into(), Value::buffer(result.bytes, 1)),
        (
            "errors".into(),
            Value::array(result.errors.into_iter().map(Value::Text).collect()),
        ),
        ("lines".into(), Value::array(lines)),
        ("labels".into(), Value::object(labels)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(input: &str) -> Vec<u8> {
        assemble(input).bytes
    }

    #[test]
    fn assembles_documented_instructions() {
        assert_eq!(words("ADD r16, r11"), [0x0b, 0x0d]);
        assert_eq!(words("loop: JMP loop"), [0x0c, 0x94, 0x00, 0x00]);
        assert_eq!(
            words("\nstart:\nLDI r16, 15\nEOR r16, r0\nBREQ start\n"),
            [0x0f, 0xe0, 0x00, 0x25, 0xe9, 0xf3]
        );
        assert_eq!(words("CALL 0xb8"), [0x0e, 0x94, 0x5c, 0x00]);
        assert_eq!(words("LDS r5, 0x150"), [0x50, 0x90, 0x50, 0x01]);
    }

    #[test]
    fn reports_out_of_range_registers() {
        let result = assemble("LDI r15, 20");
        assert!(result.bytes.is_empty());
        assert_eq!(result.errors, ["Line 0: Rd out of range: 16<>31"]);
        assert!(result.lines.is_empty());
        assert!(result.labels.is_empty());
    }

    #[test]
    fn empty_program_is_valid() {
        let result = assemble("");
        assert!(result.bytes.is_empty());
        assert!(result.errors.is_empty());
        assert!(result.lines.is_empty());
    }

    #[test]
    fn line_table_records_text_and_offset() {
        let result = assemble("ADD r16, r11");
        let entry = &result.lines[0];
        assert_eq!(entry.line, 1);
        assert_eq!(entry.text, "ADD r16, r11");
        assert_eq!(entry.byte_offset, 0);
        assert!(matches!(&entry.bytes, LineBytes::Word(word) if word == "0d0b"));
    }
}
