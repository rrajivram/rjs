// @description: Shifts, xor and masks on two variables. No allocation.
let x = 1;
let y = 0;
// @repeat
y = (y ^ (x << 3)) & 65535;
x = (x + y + {i}) & 1048575;
// @end
console.log(x, y);
