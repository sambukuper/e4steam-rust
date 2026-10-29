<div align="center">

# e4steam-rust

### Steam friends multiplayer for [elyhorizon-launcher](https://github.com/sambukuper/elyhorizon-launcher): [e4steam](https://github.com/Kamilhik/e4steam), ported to Rust

**English** · [🇷🇺 Русский](#русская-версия)

</div>

e4steam opens a Minecraft singleplayer world to Steam friends without port
forwarding or a public IP: a Steam lobby carries the invitation, Steam's P2P
networking carries the game. The original mod is Java (Fabric / Forge /
NeoForge). Horizon cannot run Java mods: it is Minecraft 26.3 rewritten in
Rust. So this project implements the mod's behaviour natively in Rust.

One command installs it into an elyhorizon-launcher checkout and builds it:

```powershell
irm https://raw.githubusercontent.com/sambukuper/e4steam-rust/main/install.ps1 | iex
```

The script finds (or clones) [sambukuper/elyhorizon-launcher](https://github.com/sambukuper/elyhorizon-launcher),
applies the port as commits with `git am` (skipped when it is already in),
builds `horizon-client` with the `steam` feature and puts the Steamworks
redistributable (`steam_api64.dll`) beside the game. Options: `-LauncherDir
<path>` for an existing checkout, `-SkipBuild` to only integrate.

You need Windows x64, git, Rust 1.90+ ([rustup](https://rustup.rs)), a copy of
Minecraft: Java Edition 26.3 (the launcher prepares the game's resources from
it), and Steam running and signed in.

## What carries over from the mod

- **App ID 480** (Valve's shared test ID), like the mod, with foreign traffic
  filtered by the lobby's Horizon metadata.
- **Access modes**: *Open to Steam Friends* (a friends lobby) and
  *Invitation Only* (a private lobby; invitations through the Steam overlay's
  invite dialog).
- **Identity**: a guest is `s_<steamid in base 36>` with the UUID the mod
  derives from the SteamID: stable per Steam account, never the name a
  client sends, and the host checks it on every join.
- One Steam networking-sockets connection per guest on the mod's own virtual
  port 480, pumped to the integrated server's LAN port.

Not ported (yet): the dedicated-server Steam mode (`d-...steam` addresses)
and the UDP voice-chat bridge. Cross-play with the Java mod is impossible in
both directions: Horizon runs none of its Java code.

In game: open a world, pause, **Play with Friends**. The Steam section opens
the world to Steam friends; guests join from a Steam invitation or the
world's `steam:<lobby id>` address. Full documentation:
[horizon-integration/INTEGRATION.md](horizon-integration/INTEGRATION.md) and
the launcher's `docs/steam-friends.md` after installing.

## Layout

| Path | What |
| --- | --- |
| `horizon-steam/` | The port as a standalone crate (`cargo test` works here on its own) |
| `horizon-integration/e4steam-port.patch` | The port as commits, `git am`-able on the launcher |
| `install.ps1` | The installer the one-liner above runs |

The launcher checkout stays the source of truth once installed: the patch is
exactly the commits it holds.

<a id="русская-версия"></a>
## 🇷🇺 Русская версия

e4steam открывает одиночный мир Minecraft друзьям из Steam без проброса
портов: приглашение несёт Steam-лобби, а сам игровой трафик идёт через P2P-сеть
Steam. Оригинальный мод написан на Java (Fabric / Forge / NeoForge). Horizon
не запускает Java-моды: это Minecraft 26.3, переписанный на Rust. Поэтому этот
проект повторяет поведение мода нативно, на Rust.

Установка в elyhorizon-launcher одной командой:

```powershell
irm https://raw.githubusercontent.com/sambukuper/e4steam-rust/main/install.ps1 | iex
```

Скрипт находит (или клонирует)
[sambukuper/elyhorizon-launcher](https://github.com/sambukuper/elyhorizon-launcher),
накатывает порт коммитами через `git am` (пропускает, если он уже стоит),
собирает `horizon-client` с фичей `steam` и кладёт библиотеку Steamworks
(`steam_api64.dll`) рядом с игрой. Параметры: `-LauncherDir <путь>` для
существующей копии, `-SkipBuild`, если нужно только внедрить без сборки.

Нужно: Windows x64, git, Rust 1.90+ ([rustup](https://rustup.rs)), своя копия
Minecraft: Java Edition 26.3 (лаунчер готовит из неё ресурсы игры) и
запущенный Steam со входом.

Как в игре: откройте мир, пауза, **Play with Friends**. Секция Steam открывает
мир друзьям Steam; гости подключаются по приглашению Steam или по адресу мира
`steam:<id лобби>`.

## Credits and license

- [e4steam](https://github.com/Kamilhik/e4steam) by Kamilchik and K2 Studio
  (Apache-2.0), the mod this port reimplements; the App ID, the identity
  scheme and the virtual port are its.
- [Horizon](https://github.com/horizon-rust/horizon): the game and the
  launcher this integrates into; this repository's code follows its GPL-3.0-only
  licence ([LICENSE](LICENSE)).
- Not an official Minecraft product; not affiliated with Valve, Mojang or
  Microsoft.
