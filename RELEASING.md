# Releasing Tokenade

Releases are built and published by GitHub Actions
(`.github/workflows/release.yml`) when a tag `v<version>` is pushed.

## One-time setup (repository settings)

Secrets (Settings → Secrets and variables → Actions), ideally scoped to an
environment named `release` with required reviewers:

| Secret | Value |
|---|---|
| `TOKENADE_UPDATE_SIGNING_KEY` | Ed25519 **private** key: the raw 32-byte seed, hex (64 chars). Its public half must equal `RELEASE_PUBKEY_HEX` in `crates/tk-base/src/signing.rs` (and `npm/install.js`); `release.mjs sign` refuses any other key. |
| `NPM_TOKEN` | npm automation token allowed to publish `@tokenade/cli` and `@tokenade/cli-*`. |

Keep an offline backup of the signing key. Losing it means existing clients
can no longer auto-update (see *Key rotation*).

## Cutting a release

1. Bump the version everywhere it is pinned:
   - `Cargo.toml` → `[workspace.package] version`, then `cargo update -w`
     so `Cargo.lock` follows;
   - `npm/package.json` → `version` **and** every `optionalDependencies`
     entry (`@tokenade/cli-*`).
2. Add a `## <version>` section at the top of `PATCHNOTES.md` (it becomes the
   release notes).
3. Commit, then tag and push:
   ```sh
   git tag v2.0.1 && git push origin v2.0.1
   ```

The workflow then:

1. checks that the tag, `Cargo.toml` and `npm/package.json` agree;
2. builds the 7 targets — `x86_64-unknown-linux-gnu` (glibc 2.31 via
   cargo-zigbuild), `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`
   (zig), `aarch64-apple-darwin`, `x86_64-apple-darwin` (macOS runner),
   `x86_64-pc-windows-gnu` (Windows runner, static CRT) and
   `aarch64-pc-windows-gnullvm` (llvm-mingw cross from Linux);
3. packs `tokenade-<version>-<target>.tar.gz` (one deterministic tar entry,
   `tokenade` / `tokenade.exe`) plus a `.sha256` for each;
4. writes `manifest.json` (schema `v: 1`, read by
   `crates/tokenade-core/src/updater.rs`) and signs it into
   `manifest.json.sig` (base64url Ed25519 over the exact bytes);
5. creates the GitHub Release (draft → upload → publish) — the updater's
   default endpoint is `releases/latest/download/`, so the new
   `manifest.json` is live for every 2.x client as soon as the release is
   published;
6. publishes the 7 `@tokenade/cli-<platform>` packages, then `@tokenade/cli`.

Re-running a failed workflow is safe: npm steps skip versions already
published, and `gh release upload --clobber` replaces assets.

### Pre-releases

A tag with a suffix (`v2.1.0-rc.1`) makes a GitHub **pre-release**
(`--latest=false`, so auto-update ignores it) and publishes npm packages
under the `next` dist-tag.

## Embedding-model asset

The semantic-search model is downloaded from Hugging Face; its fallback is a
signed tarball on a dedicated release tagged `model-<MODEL_VERSION>`
(`MODEL_VERSION` in `crates/tokenade-core/src/model_fetch.rs`), at
`https://github.com/pi-infected/tokenade/releases/download/model-<v>/model-<v>.tar.gz`
(+ `.sig`). Publish it once per model version:

```sh
# The files are in the local cache after any `tokenade semantic` run
# (Linux: ~/.cache/tokenade/model/<MODEL_VERSION>/).
V=potion-multilingual-128M-i8-v1
node scripts/ci/release.mjs pack-model --src ~/.cache/tokenade/model/$V --out out-model
node scripts/ci/release.mjs sign --key-file /path/to/signing.key out-model/model-$V.tar.gz
node scripts/ci/release.mjs verify out-model/model-$V.tar.gz
gh release create model-$V --latest=false --title "Embedding model $V" \
  --notes "Fallback for the semantic-search model (primary source: Hugging Face)." \
  out-model/model-$V.tar.gz out-model/model-$V.tar.gz.sig
```

`pack-model` checks every file against the SHA-256 baked into
`model_fetch.rs`. **Always pass `--latest=false`**: if a model release became
"latest", `releases/latest/download/manifest.json` would 404 and auto-update
would stop for every client.

## CI dry run

Actions > Release > Run workflow (or `gh workflow run release.yml --ref main`)
runs the whole release on the current commit without publishing anything:
version check, the 7 builds and their smoke tests, the artifact hand-off,
archives, manifest, signing, and `npm pack` of every package. No GitHub
Release is created and nothing reaches npm. Run it after any change to
`release.yml` or to the actions it pins (CI does not exercise this file), so
the next tag is not the first test.

## Local dry run

```sh
cargo build --release --bin tokenade
mkdir -p dist/x86_64-unknown-linux-gnu && cp target/release/tokenade dist/x86_64-unknown-linux-gnu/
node scripts/ci/release.mjs pack --dist dist --version 9.9.9 --out out
node scripts/ci/release.mjs manifest --out out --version 9.9.9 --base-url http://127.0.0.1:8000
node scripts/ci/release.mjs sign --key-file /path/to/signing.key out/manifest.json
(cd out && python3 -m http.server 8000) &
TOKENADE_UPDATE_ENDPOINT=http://127.0.0.1:8000 ./some-copy-of/tokenade upgrade --check
```

Signing with a throwaway key: pass `--any-key` to `sign` and set
`TOKENADE_UPDATE_PUBKEY_HEX` (CLI) or `TOKENADE_UPDATE_PUBKEY_HEX` +
`TOKENADE_DOWNLOADS_BASE` (npm installer) to its public half.

## Key rotation

Clients only trust the key compiled into them. To rotate: put the new public
key in `crates/tk-base/src/signing.rs` and `npm/install.js`, release that
version signed with the **old** key, wait until most clients have updated,
then switch `TOKENADE_UPDATE_SIGNING_KEY` to the new key.
