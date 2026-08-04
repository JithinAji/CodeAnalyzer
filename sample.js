// Sample JS file for testing code-analyzer
import { helper } from './helper.js';
const fs = require('fs');

// Traditional function
function add(a, b) {
    return a + b;
}

// Async function
async function fetchData(url) {
    const response = await fetch(url);
    return response.json();
}

// Arrow functions
const multiply = (a, b) => a * b;
let greet = name => `Hello, ${name}!`;

// Some code
const result = multiply(add(2, 3), 4);
console.log(result);
