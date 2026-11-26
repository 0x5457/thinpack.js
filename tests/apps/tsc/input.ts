const values: number[] = [1, 2, 3];
const doubled = values.map((value: number): number => value * 2);
const total = doubled.reduce((sum: number, value: number): number => sum + value, 0);
const text: string = new Intl.NumberFormat("en-US").format(total);
console.log(text);
