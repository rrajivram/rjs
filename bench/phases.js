// Runs a JavaScript file on Node one phase at a time, for the benchmark
// harness: `new vm.Script` parses and compiles to bytecode (V8 does both in
// one step), then `runInThisContext` executes. The program's own output goes
// to stdout; per-phase cost goes to stderr as one line:
//
//   @@phases compile_ms=1.2 exec_ms=3.4 exec_rss=0 code=65536
//
// exec_rss: growth of peak resident memory while executing, in bytes.
// code: size of V8's serialized code cache for the script, in bytes.
'use strict';
const fs = require('fs');
const vm = require('vm');

const file = process.argv[2];
const src = fs.readFileSync(file, 'utf8');
const now = () => Number(process.hrtime.bigint()) / 1e6;
// resourceUsage reports maxRSS in kilobytes.
const peakRss = () => process.resourceUsage().maxRSS * 1024;

// Node sets up console and stdout on the first console.log, which costs
// about 4ms. Do that now, with output discarded, so it isn't counted as part
// of the program's execution.
const write = process.stdout.write;
process.stdout.write = () => true;
console.log('warm-up', 1, [2]);
process.stdout.write = write;

const compileStart = now();
const script = new vm.Script(src, { filename: file });
const compileMs = now() - compileStart;

const rssAtStart = peakRss();
const execStart = now();
script.runInThisContext();
const execMs = now() - execStart;
const execRss = peakRss() - rssAtStart;

const code = script.createCachedData().length;
process.stderr.write(
  `@@phases compile_ms=${compileMs} exec_ms=${execMs} exec_rss=${execRss} code=${code}\n`,
);
