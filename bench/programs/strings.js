// @description: Builds two short strings per step. Each replaces the previous one, which becomes garbage.
let s = '';
let t = '';
// @repeat
s = 'item-' + {i};
t = s + ':' + ({i} % 7);
// @end
console.log(s, t);
