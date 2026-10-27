// @description: Creates a small array per step and drops the previous one: short-lived garbage. rjs frees each array immediately; Node leaves it to the GC.
let a = [];
let n = 0;
// @repeat
a = [{i}, n, n + 1, n + 2];
n = (n + a.length + a.indexOf(n)) % 1000003;
// @end
console.log(n, a.join(','));
