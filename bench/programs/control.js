// @description: Assigns a constant each step: the same amount of code as the other benchmarks with almost no work. Compare against it to separate the cost of parsing and compiling the program from the cost of running it.
let x = 0;
// @repeat
x = {i};
// @end
console.log(x);
