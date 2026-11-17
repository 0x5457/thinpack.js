(() => {
  try {
    if (typeof Intl?.Segmenter !== "function") throw new Error("no Segmenter");
    const probe = [...new Intl.Segmenter("en", { granularity: "grapheme" }).segment("ab")];
    if (probe.length === 2 && probe[0]?.segment === "a") return;
  } catch {
  }

  const isMark = (code) =>
    (code >= 0x0300 && code <= 0x036f) ||
    (code >= 0x1ab0 && code <= 0x1aff) ||
    (code >= 0x1dc0 && code <= 0x1dff) ||
    (code >= 0x20d0 && code <= 0x20ff) ||
    (code >= 0xfe20 && code <= 0xfe2f) ||
    (code >= 0x1f3fb && code <= 0x1f3ff) ||
    code === 0xfe0f;

  const isAsciiWord = (code) =>
    (code >= 0x30 && code <= 0x39) ||
    (code >= 0x41 && code <= 0x5a) ||
    (code >= 0x61 && code <= 0x7a) ||
    code === 0x5f;

  const isSpace = (code) =>
    code === 0x20 ||
    (code >= 0x09 && code <= 0x0d) ||
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000;

  const isWordLike = (text) => {
    for (let index = 0; index < text.length; index += 1) {
      const code = text.codePointAt(index);
      if (!isAsciiWord(code)) return false;
      if (code > 0xffff) index += 1;
    }
    return true;
  };
  function* graphemeSegments(text) {
    let ascii = true;
    for (let index = 0; index < text.length; index += 1) {
      if (text.charCodeAt(index) > 0x7f) {
        ascii = false;
        break;
      }
    }
    if (ascii) {
      for (let index = 0; index < text.length; index += 1) {
        const code = text.charCodeAt(index);
        yield {
          segment: text[index],
          index,
          input: text,
          isWordLike: isAsciiWord(code),
        };
      }
      return;
    }
    let start = 0;
    let index = 0;
    let afterJoiner = false;
    while (index < text.length) {
      const code = text.codePointAt(index);
      const joins = index > start && (isMark(code) || afterJoiner || code === 0x200d);
      if (!joins && index > start) {
        const segment = text.slice(start, index);
        yield { segment, index: start, input: text, isWordLike: isWordLike(segment) };
        start = index;
      }
      afterJoiner = code === 0x200d;
      index += code > 0xffff ? 2 : 1;
    }
    if (start < text.length) {
      const segment = text.slice(start);
      yield { segment, index: start, input: text, isWordLike: isWordLike(segment) };
    }
  }

  function* wordSegments(text) {
    let start = 0;
    let kind = 0;
    for (let index = 0; index < text.length; ) {
      const code = text.codePointAt(index);
      const nextKind = isSpace(code) ? 1 : isAsciiWord(code) ? 2 : 3;
      if (index > start && nextKind !== kind && !(kind === 3 && nextKind === 3)) {
        const segment = text.slice(start, index);
        yield { segment, index: start, input: text, isWordLike: isWordLike(segment) };
        start = index;
      }
      kind = nextKind;
      index += code > 0xffff ? 2 : 1;
    }
    if (start < text.length) {
      const segment = text.slice(start);
      yield { segment, index: start, input: text, isWordLike: isWordLike(segment) };
    }
  }

  class Segmenter {
    #granularity;
    #cache = new Map();
    #cacheCharacters = 0;

    constructor(_locales, options = {}) {
      this.#granularity = options?.granularity ?? "grapheme";
    }

    segment(input) {
      const text = String(input);
      const granularity = this.#granularity;
      let entries = text.length >= 32 ? this.#cache.get(text) : undefined;
      if (entries === undefined) {
        entries = [...(granularity === "word" ? wordSegments(text) : graphemeSegments(text))];
        if (text.length >= 32) {
          const limit = 1 << 18;
          while (this.#cache.size > 0 && this.#cacheCharacters + text.length > limit) {
            const oldest = this.#cache.keys().next().value;
            this.#cache.delete(oldest);
            this.#cacheCharacters -= oldest.length;
          }
          this.#cache.set(text, entries);
          this.#cacheCharacters += text.length;
        }
      }
      return {
        [Symbol.iterator]() {
          return entries[Symbol.iterator]();
        },
      };
    }
  }

  if (typeof globalThis.Intl !== "object" || globalThis.Intl === null) {
    globalThis.Intl = {};
  }
  Object.defineProperty(globalThis.Intl, "Segmenter", {
    value: Segmenter,
    writable: true,
    configurable: true,
  });
})();
