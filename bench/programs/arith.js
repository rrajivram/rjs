// @description: Integer arithmetic on one variable: multiply, add and modulo. No allocation.
let x = 1;
// @repeat
x = (x * 31 + {i}) % 1000003;
// @end
console.log(x);
