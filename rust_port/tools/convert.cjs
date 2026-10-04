// SPDX-License-Identifier: MIT
// AST conversion only; generated cargo tests do not require Node or TypeScript.
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const cp = require('node:child_process');
const ts = require('typescript');
const root = path.resolve(__dirname, '..');
const sourceRoot = path.resolve(root, '../avr8js/src');
const checking = process.argv.includes('--check');
const outputs = new Map();
const manifest = [];
let file;
function str(s) {
  let hashes = '#';
  while (s.includes('"' + hashes)) hashes += '#';
  return 'r' + hashes + '"' + s + '"' + hashes;
}
const vec = xs => `vec![${xs.join(',\n')}]`;
const line = n => file.getLineAndCharacterOfPosition(n.getStart(file)).line + 1;
function fail(n) { throw new Error(`${file.fileName}:${line(n)} unsupported ${ts.SyntaxKind[n.kind]}: ${n.getText(file)}`); }
function expr(n) {
  if (!n) return 'Expr::Undefined';
  if (ts.isNumericLiteral(n)) { const value = Number(n.text); return `num(${Number.isInteger(value) ? value + '.0' : value})`; }
  if (ts.isStringLiteral(n) || ts.isNoSubstitutionTemplateLiteral(n)) return `text(${str(n.text)})`;
  if (n.kind === ts.SyntaxKind.TrueKeyword || n.kind === ts.SyntaxKind.FalseKeyword) return `boolean(${n.kind === ts.SyntaxKind.TrueKeyword})`;
  if (n.kind === ts.SyntaxKind.NullKeyword) return 'Expr::Null';
  if (ts.isIdentifier(n)) {
    // Vitest control constructs and assertions must be lowered explicitly,
    // never smuggled through as unresolved backend calls (e.g. it.skip).
    if (['it','test','describe','expect','beforeEach','afterEach','beforeAll','afterAll'].includes(n.text)) fail(n);
    return n.text === 'undefined' ? 'Expr::Undefined' : `var(${str(n.text)})`;
  }
  if (ts.isParenthesizedExpression(n)) return expr(n.expression);
  if (ts.isPropertyAccessExpression(n)) return `get(${expr(n.expression)}, text(${str(n.name.text)}))`;
  if (ts.isElementAccessExpression(n)) return `get(${expr(n.expression)}, ${expr(n.argumentExpression)})`;
  if (ts.isArrayLiteralExpression(n)) return `array(${vec(n.elements.map(expr))})`;
  if (ts.isObjectLiteralExpression(n)) return `object(${vec(n.properties.map(p => {
    if (ts.isSpreadAssignment(p)) return `spread(${expr(p.expression)})`;
    if (ts.isPropertyAssignment(p)) return `entry(${str(p.name.text)}, ${expr(p.initializer)})`;
    if (ts.isShorthandPropertyAssignment(p)) return `entry(${str(p.name.text)}, ${expr(p.name)})`;
    return fail(p);
  }))})`;
  if (ts.isBinaryExpression(n)) {
    const op = n.operatorToken.getText(file);
    if (['=', '+='].includes(op)) return `assign(${expr(n.left)}, ${expr(n.right)}, ${str(op)})`;
    if (!['<','+','|','/','-','<<','*','&','==','==='].includes(op)) fail(n);
    return `binary(${str(op)}, ${expr(n.left)}, ${expr(n.right)})`;
  }
  if (ts.isPrefixUnaryExpression(n) || ts.isPostfixUnaryExpression(n)) {
    if ([ts.SyntaxKind.PlusPlusToken, ts.SyntaxKind.MinusMinusToken].includes(n.operator))
      return `update(${expr(n.operand)}, ${n.operator === ts.SyntaxKind.PlusPlusToken ? 1 : -1}, ${ts.isPrefixUnaryExpression(n)})`;
    const op = ts.tokenToString(n.operator);
    if (!['-', '+', '!', '~'].includes(op)) fail(n);
    return `unary(${str(op)}, ${expr(n.operand)})`;
  }
  if (ts.isCallExpression(n)) {
    if (n.questionDotToken) fail(n);
    return `call(${expr(n.expression)}, ${vec(n.arguments.map(expr))})`;
  }
  if (ts.isNewExpression(n)) {
    if (!ts.isIdentifier(n.expression)) fail(n);
    return `new(${str(n.expression.text)}, ${vec((n.arguments || []).map(expr))})`;
  }
  if (ts.isArrowFunction(n) || ts.isFunctionDeclaration(n)) {
    const params = n.parameters.map(p => {
      if (!ts.isIdentifier(p.name) || p.initializer) fail(p);
      return `param(${str(p.name.text)}, ${!!p.dotDotDotToken})`;
    });
    const body = ts.isBlock(n.body) ? statements(n.body.statements) : [at(n.body, `ret(${expr(n.body)})`)];
    return `function(${vec(params)}, ${vec(body)})`;
  }
  if (ts.isTemplateExpression(n)) {
    const parts = [`text(${str(n.head.text)})`];
    for (const span of n.templateSpans) parts.push(expr(span.expression), `text(${str(span.literal.text)})`);
    return `template(${vec(parts)})`;
  }
  return fail(n);
}
function at(n, action) { return `at(${line(n)}, ${action})`; }
function declarations(n) {
  return n.declarations.map(d => {
    if (ts.isIdentifier(d.name)) return at(d, `bind(${str(d.name.text)}, ${expr(d.initializer)})`);
    if (ts.isObjectBindingPattern(d.name)) {
      const fields = d.name.elements.map(e => {
        if (e.dotDotDotToken || e.initializer || !ts.isIdentifier(e.name)) fail(e);
        return `(${str(e.propertyName?.text || e.name.text)}, ${str(e.name.text)})`;
      });
      return at(d, `destructure(${vec(fields)}, ${expr(d.initializer)})`);
    }
    return fail(d);
  });
}
function body(n) { return ts.isBlock(n) ? statements(n.statements) : statements([n]); }
function statement(n) {
  if (ts.isVariableStatement(n)) return declarations(n.declarationList);
  if (ts.isFunctionDeclaration(n)) return [at(n, `bind(${str(n.name.text)}, ${expr(n)})`)];
  if (ts.isExpressionStatement(n)) {
    const e = n.expression;
    if (ts.isCallExpression(e) && ts.isPropertyAccessExpression(e.expression)) {
      let receiver = e.expression.expression;
      let negated = false;
      if (ts.isPropertyAccessExpression(receiver) && receiver.name.text === 'not') { negated = true; receiver = receiver.expression; }
      if (ts.isCallExpression(receiver) && ts.isIdentifier(receiver.expression) && receiver.expression.text === 'expect') {
        const matcher = e.expression.name.text;
        if (!['toEqual','toBe','toBeGreaterThanOrEqual','toHaveBeenCalled','toHaveBeenCalledWith','toHaveBeenCalledTimes'].includes(matcher) || receiver.arguments.length !== 1) fail(n);
        return [at(n, `check(${expr(receiver.arguments[0])}, ${str(matcher)}, ${negated}, ${vec(e.arguments.map(expr))})`)];
      }
    }
    return [at(n, `eval(${expr(e)})`)];
  }
  if (ts.isReturnStatement(n)) return [at(n, `ret(${expr(n.expression)})`)];
  if (ts.isThrowStatement(n)) return [at(n, `throw(${expr(n.expression)})`)];
  if (ts.isIfStatement(n)) return [at(n, `if_(${expr(n.expression)}, ${vec(body(n.thenStatement))}, ${vec(n.elseStatement ? body(n.elseStatement) : [])})`)];
  if (ts.isForStatement(n)) return [at(n, `for_(${vec(ts.isVariableDeclarationList(n.initializer) ? declarations(n.initializer) : [at(n, `eval(${expr(n.initializer)})`)] )}, ${expr(n.condition)}, ${expr(n.incrementor)}, ${vec(body(n.statement))})`)];
  if (ts.isForOfStatement(n)) {
    if (!ts.isVariableDeclarationList(n.initializer) || n.initializer.declarations.length !== 1 || !ts.isIdentifier(n.initializer.declarations[0].name)) fail(n);
    return [at(n, `for_of(${str(n.initializer.declarations[0].name.text)}, ${expr(n.expression)}, ${vec(body(n.statement))})`)];
  }
  return fail(n);
}
const statements = ns => Array.from(ns).flatMap(statement);
function countAssertions(n) {
  let count = 0;
  function visit(node) {
    if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === 'expect') count++;
    ts.forEachChild(node, visit);
  }
  visit(n); return count;
}
function discover(dir) {
  return fs.readdirSync(dir, {withFileTypes:true}).flatMap(e => {
    const p = path.join(dir,e.name);
    return e.isDirectory() ? discover(p) : p.endsWith('.spec.ts') ? [p] : [];
  }).sort();
}
for (const source of discover(sourceRoot)) {
  const sourceText = fs.readFileSync(source, 'utf8');
  file = ts.createSourceFile(source, sourceText, ts.ScriptTarget.Latest, true);
  if (file.parseDiagnostics.length) throw new Error(`Parse errors in ${source}`);
  const relative = path.relative(path.dirname(sourceRoot), source).replaceAll(path.sep, '/');
  const moduleName = path.relative(sourceRoot, source).replace('.spec.ts','').replaceAll(/[/\\-]/g,'_');
  // Suites whose native Rust backend is implemented run by default; the rest stay
  // gated so a green `cargo test` never implies untested simulator coverage.
  const IMPLEMENTED = new Set(['utils_assembler', 'cpu_cpu', 'cpu_interrupt', 'cpu_instruction', 'peripherals_clock', 'peripherals_gpio', 'peripherals_timer', 'peripherals_timer_attiny']);
  const cases = [];
  const imports = [];
  function collect(nodes, names = [], outerSetup = [], outerHooks = []) {
    const setup = [...outerSetup]; const hooks = [...outerHooks];
    // Suite declarations/functions are evaluated once in the source; their pure
    // setup is replayed into a fresh lexical environment for each Rust test.
    for (const n of nodes) {
      if (ts.isImportDeclaration(n)) {
        if (n.moduleSpecifier.text !== 'vitest') imports.push(...n.importClause.namedBindings.elements.filter(e => !e.isTypeOnly && !['AVRClockEventCallback'].includes(e.name.text)).map(e=>e.name.text));
      } else if (ts.isTypeAliasDeclaration(n)) { /* compile-time-only */ }
      else if (ts.isExpressionStatement(n) && ts.isCallExpression(n.expression) && ts.isIdentifier(n.expression.expression) && ['describe','it','beforeEach'].includes(n.expression.expression.text)) {
        if (n.expression.expression.text === 'beforeEach') hooks.push(...body(n.expression.arguments[0].body));
      } else setup.push(...statement(n));
    }
    for (const n of nodes) {
      if (!(ts.isExpressionStatement(n) && ts.isCallExpression(n.expression) && ts.isIdentifier(n.expression.expression))) continue;
      const c = n.expression; const kind = c.expression.text;
      if (kind === 'describe') collect(c.arguments[1].body.statements, [...names,c.arguments[0].text],setup,hooks);
      if (kind === 'it') {
        const name = [...names,c.arguments[0].text].join(' / ');
        const id = `case_${String(cases.length+1).padStart(3,'0')}_${c.arguments[0].text.toLowerCase().replaceAll(/[^a-z0-9]+/g,'_').replaceAll(/^_|_$/g,'').slice(0,100)}`;
        cases.push({id,name,line:line(n),assertions:countAssertions(c.arguments[1].body),setup:[...setup,...hooks],body:body(c.arguments[1].body)});
      }
    }
  }
  collect(file.statements);
  const sourceAssertions = countAssertions(file);
  if (cases.reduce((n,c)=>n+c.assertions,0) !== sourceAssertions) throw new Error(`Unassigned assertion in ${source}`);
  if (!cases.length) throw new Error(`No cases converted in ${source}`);
  let rust = '// SPDX-License-Identifier: MIT\n// Copyright (c) Uri Shaked and contributors\n// Generated by tools/convert.cjs. Source: '+relative+'\nuse crate::scenario::*;\n';
  rust += `pub const IMPORTS: &[&str] = &[${imports.map(str).join(',')}];\n`;
  rust += `pub fn cases() -> Vec<Case> { vec![${cases.map(c=>c.id+'()').join(',')}] }\n`;
  const setups = [...new Set(cases.map(c => vec(c.setup)))];
  setups.forEach((setup, i) => { rust += `\nfn setup_${i}() -> Vec<Step> { ${setup} }\n`; });
  for (const c of cases) {
    rust += `\npub fn ${c.id}() -> Case { Case { source: ${str(relative)}, name: ${str(c.name)}, line: ${c.line}, assertions: ${c.assertions}, setup: setup_${setups.indexOf(vec(c.setup))}(), body: ${vec(c.body)} } }\n`;
    const gate = IMPLEMENTED.has(moduleName) ? '' : '#[ignore = "native peripheral implementation pending"]\n';
    rust += `#[test]\n${gate}fn ${c.id}_native() { crate::run_native(${c.id}()); }\n`;
  }
  outputs.set(`src/suites/${moduleName}.rs`, rust);
  manifest.push({source:relative,sha256:crypto.createHash('sha256').update(sourceText).digest('hex'),module:moduleName,imports,cases:cases.map(({id,name,line,assertions})=>({id,name,line,assertions}))});
}
outputs.set('src/suites/mod.rs', '// Generated by tools/convert.cjs.\n'+manifest.map(m=>`pub mod ${m.module};`).join('\n')+'\npub fn all_cases() -> Vec<crate::Case> {\nlet mut result = Vec::new();\n'+manifest.map(m=>`result.extend(${m.module}::cases());`).join('\n')+'\nresult\n}\n');
outputs.set('conversion-manifest.json', JSON.stringify({format:1,suites:manifest.length,cases:manifest.reduce((a,m)=>a+m.cases.length,0),assertions:manifest.reduce((a,m)=>a+m.cases.reduce((s,c)=>s+c.assertions,0),0),files:manifest},null,2)+'\n');
// Compare formatted output, so rustfmt does not make regeneration checks noisy.
for (const [relative, content] of outputs) {
  let formatted = content;
  if (relative.endsWith('.rs')) formatted = cp.execFileSync('rustfmt',['--edition','2021','--emit','stdout'],{input:content,encoding:'utf8',maxBuffer:10*1024*1024});
  const target = path.join(root,relative);
  if (checking) {
    if (!fs.existsSync(target) || fs.readFileSync(target,'utf8') !== formatted) throw new Error(`Stale conversion: ${relative}`);
  } else { fs.mkdirSync(path.dirname(target),{recursive:true}); fs.writeFileSync(target,formatted); }
}
const expectedModules = new Set(manifest.map(m=>m.module+'.rs').concat('mod.rs'));
for (const filename of fs.readdirSync(path.join(root,'src/suites'))) if (filename.endsWith('.rs') && !expectedModules.has(filename)) throw new Error(`Obsolete generated suite: ${filename}`);
console.log(`${checking ? 'Verified' : 'Converted'} ${manifest.length} suites, ${manifest.reduce((a,m)=>a+m.cases.length,0)} cases.`);
