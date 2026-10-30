// @description: A fixed mixed workload: prime sieve, insertion sort, statistics and a histogram, 30x30 matrix multiply, FizzBuzz strings, and destructuring.
// Sieve of Eratosthenes: primes below N.
const N = 20000;
let isPrime = [];
for (let i = 0; i < N; i = i + 1) { isPrime[i] = true; }
isPrime[0] = false;
isPrime[1] = false;
for (let p = 2; p * p < N; p = p + 1) {
  if (isPrime[p]) {
    for (let m = p * p; m < N; m = m + p) { isPrime[m] = false; }
  }
}
let primes = [];
for (let i = 0; i < N; i = i + 1) { if (isPrime[i]) { primes.push(i); } }

// Insertion sort of pseudo-random data (ZX81 linear congruential generator).
let data = [];
let seed = 42;
for (let i = 0; i < 1500; i = i + 1) {
  seed = (seed * 75 + 74) % 65537;
  data.push(seed % 1000);
}
for (let i = 1; i < data.length; i = i + 1) {
  let key = data[i];
  let j = i - 1;
  for (; j >= 0 && data[j] > key; j = j - 1) { data[j + 1] = data[j]; }
  data[j + 1] = key;
}

// Statistics and a histogram over the sorted data.
let sum = 0;
let min = data[0];
let max = data[0];
for (let i = 0; i < data.length; i = i + 1) {
  let v = data[i];
  sum += v;
  min = v < min ? v : min;
  max = v > max ? v : max;
}
let mean = sum / data.length;
let median = data[data.length / 2];
let buckets = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
for (let i = 0; i < data.length; i = i + 1) {
  let b = (data[i] - data[i] % 100) / 100;
  buckets[b] += 1;
}

// 30x30 matrix multiplication with nested arrays.
let n = 30;
let A = [];
let B = [];
for (let i = 0; i < n; i = i + 1) {
  let ra = [];
  let rb = [];
  for (let j = 0; j < n; j = j + 1) { ra.push((i + j) % 7); rb.push((i * j) % 5); }
  A.push(ra);
  B.push(rb);
}
let C = [];
for (let i = 0; i < n; i = i + 1) {
  let row = [];
  for (let j = 0; j < n; j = j + 1) {
    let acc = 0;
    for (let k = 0; k < n; k = k + 1) { acc += A[i][k] * B[k][j]; }
    row.push(acc);
  }
  C.push(row);
}
let trace = 0;
for (let i = 0; i < n; i = i + 1) { trace += C[i][i]; }

// FizzBuzz built as strings.
let out = [];
for (let i = 1; i <= 100; i = i + 1) {
  out.push(i % 15 == 0 ? 'FizzBuzz' : i % 3 == 0 ? 'Fizz' : i % 5 == 0 ? 'Buzz' : '' + i);
}

// Destructuring.
let [first, second, ...rest] = primes;

console.log(primes.length, primes[primes.length - 1], first, second, rest.length);
console.log(sum, min, max, mean, median, buckets.join(' '));
console.log(trace, C[n - 1][n - 1]);
console.log(out.length, out[14], out.slice(0, 15).join(','));
