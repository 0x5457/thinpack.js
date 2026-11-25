# App fixtures

One npm project per program in the README's comparison tables, measured by `cargo xtask apps`.
`package.json` pins the version and `package-lock.json` the transitive tree, so an artifact
only changes size when the tree or the runtime does. The command installs each fixture with
`npm ci --ignore-scripts` and packs it with `--rebuild`.

## Adding or bumping an app

The app table, its entry point and the files `--include` has to carry live in
`xtask/src/apps.rs`; the fixture directory is named after the artifact.

```sh
cd tests/apps/prettier
npm install prettier@<version> --package-lock-only
cargo xtask apps --write-baseline        # records every app, reviewed as a diff
```

`--write-baseline` refuses `--only`: a partial measurement must not shrink the baseline to
the apps it happened to run.

Record the baseline on the reference runner (`ubuntu-24.04` with the pinned toolchain). The
gate compares the target triple, backend and profile, but a same-triple host with another
compiler or libc still produces different bytes, and the 1% tolerance is meant for runner
image updates, not for a different build environment.

The Size workflow also reports a local `hello` fixture (`console.log('hello')`).
It builds the minimal template, runs the executable, and reports its exact raw
and gzip sizes separately. It does not install npm packages or affect the app baseline.
