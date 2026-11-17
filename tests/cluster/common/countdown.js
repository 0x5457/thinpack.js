'use strict';

module.exports = class Countdown {
  #remaining;
  #callback;
  constructor(remaining, callback) {
    this.#remaining = remaining;
    this.#callback = callback;
  }
  dec() {
    if (--this.#remaining === 0) this.#callback();
    return this.#remaining;
  }
};
