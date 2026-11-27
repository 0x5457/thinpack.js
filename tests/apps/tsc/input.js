const values = [1, 2, 3];
const doubled = values.map((value) => value * 2);
const total = doubled.reduce((sum, value) => sum + value, 0);
const text = new Intl.NumberFormat("en-US").format(total);
console.log(text);
