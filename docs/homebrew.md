# Homebrew packaging

This repository doubles as a custom Homebrew tap through its `Formula/` directory:

```bash
brew tap spill-ai/spill https://github.com/spill-ai/spill
brew trust --formula spill-ai/spill/spill
brew install spill
spill install cursor
```

The explicit URL is necessary because Homebrew's default shorthand would look for `spill-ai/homebrew-spill`. Users only tap once. Homebrew 6+ also needs the formula-scoped trust command above for installation by short name; omit that command on older versions. A fully qualified `brew install spill-ai/spill/spill` grants trust to that formula automatically. A bare `brew install spill` on a machine without this tap requires a future accepted submission to `homebrew/core`; adding a formula to GitHub does not register it globally.

The repository and pinned commit must be reachable on GitHub. Public installation requires a public repository; private repositories require the user's own Git credentials. No credentials belong in the formula.

## Stable builds

`Formula/spill.rb` pins an immutable Git revision and a package version. Homebrew builds that revision with `cargo install --locked`, including bundled DuckDB. There are no prebuilt bottles yet. `Cargo.lock` must be committed in the pinned revision.

To publish an update:

1. Run formatting, Clippy, Rust tests and `scripts/smoke.py` as described in the README.
2. Update `Cargo.toml` and `Cargo.lock` to the new package version and commit the tested source.
3. Set the formula's `revision` to that full commit SHA and its `version` to the package version. If changing the packaged source without a version bump, increment Homebrew's formula `revision` integer.
4. Commit the formula change and push both commits to `main`.
5. Verify CI, including the Homebrew installation test. Users can then run `brew update && brew upgrade spill`.

Keeping the package pin separate from the formula commit avoids a self-referential revision. Homebrew's `head` option remains available for development builds.

## Local verification

After publishing the pinned commit:

```bash
brew tap spill-ai/spill https://github.com/spill-ai/spill
brew install --build-from-source spill-ai/spill/spill
brew test spill-ai/spill/spill
```

The formula test uses Homebrew's temporary test home. It checks installation, spilling a large result, SQL access to the saved rows, and uninstall. It does not modify the user's Cursor configuration.

See [Homebrew's tap documentation](https://docs.brew.sh/How-to-Create-and-Maintain-a-Tap) for custom remote URLs and formula resolution.

The current trust behavior is documented in [Homebrew Tap Trust](https://docs.brew.sh/Tap-Trust).
