// @description: An if/else and a conditional expression per step.
let x = 7;
let up = 0;
let down = 0;
// @repeat
if (x % 3 == 0) { up = up + 1; } else { down = down + 1; }
x = x % 2 == 0 ? x / 2 + {i} % 7 : (x * 3 + 1) % 1000003;
// @end
console.log(x, up, down);
