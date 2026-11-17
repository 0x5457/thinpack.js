(() => {
  const readline = require("node:readline");

  const { AbortError } = require("node:thinpack/internal/errors");

  class Interface extends readline.Interface {
    question(query, options = {}) {
      const signal = options?.signal;
      if (signal?.aborted) return Promise.reject(new AbortError());
      const answer = super.question(query);
      if (signal === undefined) return answer;
      return new Promise((resolve, reject) => {
        const onAbort = () => reject(new AbortError());
        signal.addEventListener("abort", onAbort, { once: true });
        answer.then(
          (value) => { signal.removeEventListener("abort", onAbort); resolve(value); },
          (error) => { signal.removeEventListener("abort", onAbort); reject(error); },
        );
      });
    }
  }

  const createInterface = (options, output) => {
    if (options?.input !== undefined) {
      const iface = new Interface(options.input, options.output ?? null);
      if (options.prompt !== undefined) iface.setPrompt(options.prompt);
      if (Array.isArray(options.history)) iface.history = options.history;
      return iface;
    }
    return new Interface(options, output ?? null);
  };

  module.exports = { Interface, createInterface };
})();
