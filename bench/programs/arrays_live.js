// @description: Creates a small array per step and keeps every one alive in its own variable, so memory grows with N in both engines.
let n = 0;
// @repeat
let a{i} = [{i}, {i} + 1, {i} + 2];
n = n + a{i}.length;
// @end
console.log(n);
