// @description: Enters a block with its own `let` each step. In rjs every block execution creates a scope.
let x = 0;
// @repeat
{ let t = x + {i}; x = t % 9973; }
// @end
console.log(x);
