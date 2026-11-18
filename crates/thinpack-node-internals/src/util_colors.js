(() => {
  const styles = {
    reset: "\u001b[0m",
    bold: "\u001b[1m",
    dim: "\u001b[2m",
    italic: "\u001b[3m",
    underline: "\u001b[4m",
    inverse: "\u001b[7m",
    hidden: "\u001b[8m",
    strikethrough: "\u001b[9m",
    black: "\u001b[30m",
    red: "\u001b[31m",
    green: "\u001b[32m",
    yellow: "\u001b[33m",
    blue: "\u001b[34m",
    magenta: "\u001b[35m",
    cyan: "\u001b[36m",
    white: "\u001b[37m",
    gray: "\u001b[90m",
    grey: "\u001b[90m",
    bgBlack: "\u001b[40m",
    bgRed: "\u001b[41m",
    bgGreen: "\u001b[42m",
    bgYellow: "\u001b[43m",
    bgBlue: "\u001b[44m",
    bgMagenta: "\u001b[45m",
    bgCyan: "\u001b[46m",
    bgWhite: "\u001b[47m",
  };

  const forcedColor = () => {
    const value = process.env.FORCE_COLOR;
    if (value === undefined || value === "") return undefined;
    return value !== "0" && value !== "false";
  };

  const shouldColorize = (stream) => {
    const forced = forcedColor();
    if (forced !== undefined) return forced;
    if (process.env.NO_COLOR !== undefined) return false;
    return Boolean(stream && stream.isTTY);
  };

  module.exports = {
    ...styles,
    hasColors: shouldColorize(process.stdout),
    refresh() {},
    shouldColorize,
  };
})();
