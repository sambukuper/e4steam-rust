# Applying the port to elyhorizon-launcher

`e4steam-port.patch` is `git format-patch` output: three commits carrying the
whole port, exactly the ones the launcher checkout holds after installing.

- base: `7b8d9d73b` (`fix: show Ely.by skins, including other players' on a
  server`) on **sambukuper/elyhorizon-launcher** — apply the patch onto that
  commit, or any descendant that doesn't touch the same files yet;
- `0001` **Add horizon-steam**: the crate (runtime, host share, guest join,
  identity, join guard) and its entry in the workspace;
- `0002` **Open the world to Steam friends from Play with Friends**: the
  client integration (the Steam section, `steam:` joins, overlay invites, the
  login-name override) and the `steam` feature;
- `0003` **Document Steam sharing**: `docs/steam-friends.md` and the doc
  indexes.

By hand:

```sh
git clone https://github.com/sambukuper/elyhorizon-launcher.git
cd elyhorizon-launcher
git am --3way /path/to/e4steam-rust/horizon-integration/e4steam-port.patch
cargo build --release -p horizon-client --features steam
# then copy steam_api64.dll from the steamworks-sys crate package
# (lib/steam/redistributable_bin/win64/) beside target/release/horizon-client.exe
```

`install.ps1` does the same, plus the build and the dll, in one command.

Once applied, `crates/horizon-steam` in the launcher is the source of truth;
the standalone crate in this repository's `horizon-steam/` is the same code
with Horizon's `md5` and UUID helpers inlined so it builds on its own.
