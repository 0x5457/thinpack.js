(() => {
  const { validateInteger } = require("node:thinpack/internal/validators");

  const COLORS_2 = 1;
  const COLORS_16 = 4;
  const COLORS_256 = 8;
  const COLORS_16M = 24;
  const TERM_ENVS = {
    eterm: COLORS_16, cons25: COLORS_16, console: COLORS_16, cygwin: COLORS_16,
    dtterm: COLORS_16, gnome: COLORS_16, hurd: COLORS_16, jfbterm: COLORS_16,
    konsole: COLORS_16, kterm: COLORS_16, mlterm: COLORS_16, mosh: COLORS_16M,
    putty: COLORS_16, st: COLORS_16, "rxvt-unicode-24bit": COLORS_16M,
    terminator: COLORS_16M, "xterm-kitty": COLORS_16M,
  };
  const CI_ENVS = [
    ["APPVEYOR", COLORS_256], ["BUILDKITE", COLORS_256],
    ["CIRCLECI", COLORS_16M], ["DRONE", COLORS_256],
    ["GITEA_ACTIONS", COLORS_16M], ["GITHUB_ACTIONS", COLORS_16M],
    ["GITLAB_CI", COLORS_256], ["TRAVIS", COLORS_256],
  ];
  const TERM_ENVS_REG_EXP = [
    /ansi/, /color/, /linux/, /direct/, /^con[0-9]*x[0-9]/, /^rxvt/,
    /^screen/, /^xterm/, /^vt100/, /^vt220/,
  ];

  const getColorDepth = (environment = process.env) => {
    if (environment.FORCE_COLOR !== undefined) {
      switch (environment.FORCE_COLOR) {
        case "":
        case "1":
        case "true":
          return COLORS_16;
        case "2":
          return COLORS_256;
        case "3":
          return COLORS_16M;
        default:
          return COLORS_2;
      }
    }

    if (
      (environment.NODE_DISABLE_COLORS !== undefined && environment.NODE_DISABLE_COLORS !== "") ||
      (environment.NO_COLOR !== undefined && environment.NO_COLOR !== "") ||
      environment.TERM === "dumb"
    ) {
      return COLORS_2;
    }

    if (environment.TMUX) return COLORS_16M;
    if (Object.hasOwn(environment, "TF_BUILD") && Object.hasOwn(environment, "AGENT_NAME")) {
      return COLORS_16;
    }

    if (Object.hasOwn(environment, "CI")) {
      for (const [envName, colors] of CI_ENVS) {
        if (Object.hasOwn(environment, envName)) return colors;
      }
      if (environment.CI_NAME === "codeship") return COLORS_256;
      return COLORS_2;
    }

    if (Object.hasOwn(environment, "TEAMCITY_VERSION")) {
      return /^(9\.(0*[1-9]\d*)\.|\d{2,}\.)/.test(environment.TEAMCITY_VERSION)
        ? COLORS_16
        : COLORS_2;
    }

    switch (environment.TERM_PROGRAM) {
      case "iTerm.app":
        return !environment.TERM_PROGRAM_VERSION || /^[0-2]\./.test(environment.TERM_PROGRAM_VERSION)
          ? COLORS_256
          : COLORS_16M;
      case "HyperTerm":
      case "MacTerm":
        return COLORS_16M;
      case "Apple_Terminal":
        return COLORS_256;
    }

    const colorterm = environment.COLORTERM;
    if (colorterm === "truecolor" || colorterm === "24bit") return COLORS_16M;

    const term = environment.TERM;
    if (term) {
      if (/truecolor/.test(term)) return COLORS_16M;
      if (/^xterm-256/.test(term)) return COLORS_256;
      const name = term.toLowerCase();
      if (Object.hasOwn(TERM_ENVS, name)) return TERM_ENVS[name];
      if (TERM_ENVS_REG_EXP.some((pattern) => pattern.test(name))) return COLORS_16;
    }

    return colorterm ? COLORS_16 : COLORS_2;
  };

  const hasColors = (count, environment) => {
    if (
      environment === undefined &&
      (count === undefined || (typeof count === "object" && count !== null))
    ) {
      environment = count;
      count = 16;
    } else {
      validateInteger(count, "count", 2);
    }
    return count <= 2 ** getColorDepth(environment);
  };

  module.exports = { getColorDepth, hasColors };
})();
