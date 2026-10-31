# Next steps

## Functions

Done: declarations (hoisted), calls (an explicit frame stack), parameters (defaults, rest,
destructuring, via `arguments`), `return`, recursion, lexical scoping through
the defining scope, functions as values.

1. **Callbacks from built-ins.** JS calls run as frames in `run_frame`'s loop,
   not as Rust recursion, so depth is limited only by `MAX_CALL_DEPTH`
   (10,000, like Node). Built-ins that call JS (`map`, `forEach`, `sort` with
   a comparator, `Array.from` with a map function) can use
   `JSEngine::call_function`, which runs a nested loop and so does recurse in
   Rust; fine unless callbacks nest thousands deep. Generators and async will
   need a frame that can be taken off the stack and resumed later.
2. **Closures.** Calling a function after the scope it was created in has
   ended is `NotSupported` (the defining scope is held weakly). Needs
   per-variable capture; design it together with variable slots (performance
   item 2 below).
3. **`this` and method calls.** A JS function stored on an object is called
   without the object.
4. **Function expressions and arrows.** `let f = function () {}` panics with
   `todo!()` (`engine.rs` `emit_expression`); emitting `MakeFunction` there
   with `emit_function` should cover it. Arrows also need lexical `this`.
   Async and generator functions are `NotSupported`.

## Statements and expressions

Done: `while`, `do-while`, `for-of` (arrays and strings), `break`/`continue`
with labels, `switch`, empty and `debugger` statements, unary operators,
`++`/`--`, sequences.

Left, roughly easiest first:

1. **String properties.** `'abc'.length` panics (`todo!()` in
   `JsValue::get_prop` for non-objects).
2. **Optional chaining** (`a?.b`, `a?.[i]`, `f?.()`): parsed, but the
   `optional` flag is ignored. A jump over the access when the object is
   `null`/`undefined`.
3. **Object literals and property assignment** (`{ a: 1 }`, `obj.x = v`,
   `obj.x++`): `ObjectPrimitive` and `SetAttr` exist; decide how to create an
   object (a built-in like `Array.new`, or a `MakeObject` instruction).
4. **Spread** in array literals and calls.
5. **`this` and symbols, what's left.** `Symbol()`, `Symbol.iterator`,
   `Symbol.asyncIterator` and `call`/`apply`/`bind` work, and each call's
   scope has a `this` variable (the object for `obj.f()`, otherwise
   undefined). Left: `Expr::This` (emit `Operand::Var("this")`); `this` for
   `obj[k]()` calls (only `obj.f()` passes its object); symbol descriptions
   (`Symbol('x')` keeps no description); `typeof Symbol` is "object", not
   "function". Adding `this` to every call's scope made `fib(25)` about 15%
   slower: add `this` and `arguments` only for functions that use them (a
   compile-time check of the body), or let variable slots (performance
   item 2) make it free.
6. **BigInt literals** (`1n`) aren't parsed; the VM's BigInt arithmetic is
   only reachable from Rust.
7. **Exceptions** (`throw`, `try`/`catch`/`finally`): unwinding through
   frames and their block scopes. `NotSupported` for now, as are `for-in`,
   classes and modules.

## Interpreter performance (revisit after functions and objects)

Done so far: blocks that declare nothing no longer create a scope, registers
are an array instead of a `HashMap`, and scope variable maps use FxHash.
`JsValue` is 24 bytes, down from 48: functions are an `Rc` handle and BigInts
an `Rc<i128>` (a compile-time assert in `primitive/mod.rs` keeps it there).
That made `complex.js` about 9% faster and `fib(25)` about 20% (measured on
a loaded machine).

Execution time after those changes (release build, `rjs_phases`, median of 5;
`node --jitless` for comparison):

| program | rjs | node --jitless |
|---|---|---|
| counting loop, braced body (1M iterations) | 121 ms | 11 ms |
| counting loop, bare (1M iterations) | 96 ms | 6 ms |
| fill 1M-element array with `push`, sum with `a[i]` | 418 ms | 38 ms |
| `bench/programs/complex.js` (8.2M instructions) | 348 ms | 14 ms |

The `complex.js` figure is without per-instruction statistics; with them
(`rjs --verbose`) it's about 620 ms (see "Instrumentation" below).

### Where the time goes

Per-instruction statistics for `complex.js` (times include roughly 15 ns of
measurement per instruction; `Jump`, which does almost nothing, shows the
floor):

| instruction | count | each | share |
|---|---|---|---|
| Binary | 3.05 M | 45 ns | 36% |
| GetIndex | 1.24 M | 94 ns | 30% |
| Declare | 1.14 M | 42 ns | 13% |
| SetIndex | 0.61 M | 72 ns | 11% |
| Set | 0.75 M | 20 ns | 4% |
| JumpIf / Jump | 1.37 M | 15–16 ns | 6% |

`Binary`, `GetIndex` and `SetIndex` spend most of their time finding variables
by name, which item 2 fixes. `Declare` is compiler temporaries, which item 1
fixes.

Re-measured after adding functions and the frame stack (2026-10-28,
`rjs --verbose`, median of 3; 527 ms of instructions measured):

| instruction | count | each | share |
|---|---|---|---|
| Binary | 3.05 M | 62 ns | 36% |
| GetIndex | 1.24 M | 132 ns | 31% |
| Declare | 1.14 M | 56 ns | 12% |
| SetIndex | 0.61 M | 97 ns | 11% |
| Set | 0.75 M | 25 ns | 4% |
| Jump | 0.68 M | 21 ns | 3% |
| JumpIf | 0.69 M | 21 ns | 3% |
| Call / GetAttr / Hoist / Init | 0.01 M each | 25–128 ns | <1% |

Counts and shares match the earlier table; every instruction, `Jump`
included, is 35–40% slower. A uniform slowdown points at the machine, not the
code: this run shared the CPU with Zed (~95%) and a stray `rjs` stuck on an
old native stack overflow (~55%). Under the same load `rjs_phases` gave
475 ms against 348 ms before. Re-run on an idle machine before reading
anything into the per-instruction times. The frame stack itself costs nothing
measurable: under the same load, a build with calls recursing in Rust ran
`complex.js` in 474 ms vs 475 ms, and `fib(25)` in 161 ms vs 164 ms.

### Planned optimizations

1. **Give temporaries their own slots.** Every compiler temporary (`__t0`,
   `__t1`, ...) is currently a real scope variable: `Declare` hashes its name
   and allocates a `Variable`. In `complex.js` that's 1.14M declarations,
   about 13% of execution time, mostly two per iteration of the insertion
   sort's inner loop (the left side of `j >= 0 && data[j] > key`, and the index
   in `data[j + 1] = data[j]`). Store temporaries in a `Vec<JsValue>` on the
   engine, addressed by an `Operand::Temp(n)`, instead. Small and
   self-contained; can be done before functions.
2. **Resolve variables to slots at compile time.** Replace `Operand::Var(name)`
   with something like `Operand::Slot { depth, index }`: walk up `depth`
   scopes, then index a `Vec`. No hashing or string comparison at runtime.
   Expected to be the largest remaining gain. Needs a compile-time model of
   scopes in the emitter, and has to fit with hoisting and with closures
   capturing variables (`Rc<RefCell<Variable>>` handles), so design it together
   with function support.
3. **Fused compare-and-jump instructions.** A loop test is currently a
   `Binary` into AX followed by a `JumpIf`; a single `JumpIfLt a, b, target`
   (and friends) halves the per-iteration instructions for the test.
4. **One scope per loop for loop bodies that declare variables.** A body like
   `{ let a = [i, i]; ... }` creates and frees a scope every iteration. Create it
   once per loop and re-run the body's `Hoist` instructions each iteration;
   closures from earlier iterations keep the variables they captured, since
   re-hoisting creates new `Variable`s.
5. **Tidy `GetIndex`/`SetIndex`.** They call `as_array()` (a dynamic call)
   twice, and write their result through the general-purpose `set`.

Before starting, profile a loop benchmark (release builds include debug info;
e.g. `samply record target/release/rjs_phases file.js`) to confirm the order.

### Instrumentation

The per-instruction statistics in `Instruction::exec` cost about 34 ns per
instruction, nearly doubling execution time. They're only gathered and
printed when `JSEngine::verbose` is on (`rjs --verbose file.js`), so
benchmarks don't include them.

- Keep the counts; get timings from a sampling profiler instead.
- `BeginScope`/`EndScope` are handled in `run` before `exec`, so they always
  show 0.
- The `us` field holds nanoseconds.
