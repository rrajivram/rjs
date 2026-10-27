// @description: A working array that grows with push and is trimmed with slice when it gets long, turning the old array into garbage.
let arr = [];
let sum = 0;
// @repeat
arr.push({i}, {i} % 13);
if (arr.length > 64) { arr = arr.slice(32); }
sum = (sum + arr.indexOf({i} % 13) + arr.length) % 1000003;
// @end
console.log(sum, arr.length);
