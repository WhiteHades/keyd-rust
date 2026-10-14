<div align="center">
<pre>
 _                  _                    _
| | _____ _   _  __| |      _ __ _   _ ___| |_
| |/ / _ \ | | |/ _` |_____| '__| | | / __| __|
|   &lt;  __/ |_| | (_| |_____| |  | |_| \__ \ |_
|_|\_\___|\__, |\__,_|     |_|   \__,_|___/\__|
          |___/
</pre>

![Linux](https://img.shields.io/badge/Linux-grey)
![Rust](https://img.shields.io/badge/Rust-1.98%2B-orange)
[![MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
</div>

keyd-rust remaps Linux keyboards in Wayland, X11 and the console.
It is a Rust rewrite of [keyd](https://github.com/rvaiya/keyd).

## Differences from keyd

| Comparison | Original keyd | keyd-rust |
| --- | --- | --- |
| Implementation | C daemon, some Python tools | Rust, no C engine or bridge |
| Double-tap action | No `overloadd` | Tap, double tap, immediate held layer; configurable timing |
| Device removal | Keeps removed-device descriptors | Closes them |
| Existing mappings | Original trace fixtures | All 94 original traces passed |

Linux kernel checks passed on x86_64, i686 and ARM64, including x86_64 musl.
Lower latency, memory use or binary size have not been established.

## Install

You need Rust 1.98+, Cargo, make and scdoc. On Arch Linux:

```sh
git clone https://github.com/WhiteHades/keyd-rust.git
cd keyd-rust
cargo fetch --locked
bash packaging/build
sudo pacman -U .tmp/package/*.pkg.tar.zst
```

Elsewhere, use `make && make man && sudo make install`.
The daemon needs `getent` and access to `/dev/input` and `/dev/uinput`.
Back up your configuration and stop/remove original keyd before starting keyd-rust.

## Configure

Create `/etc/keyd-rust/default.conf`, or reuse a keyd config:

```sh
sudo install -d /etc/keyd-rust && sudoedit /etc/keyd-rust/default.conf
```

```ini
[ids]
*

[main]
esc = capslock
capslock = esc
rightcontrol = layer(arrows)

[arrows]
i = up
j = left
k = down
l = right
```

The example swaps Escape/Caps Lock and makes Right Control+i/j/k/l arrows.
Validate before starting:

```sh
sudo keyd-rust check && sudo systemctl enable --now keyd-rust
```

After edits, run `sudo keyd-rust check && sudo keyd-rust reload`.
Without systemd, run `keyd-rust` as root through your init system.

## Syntax

Files use `.conf` and start with `[ids]`. `sudo keyd-rust monitor` shows IDs and
pressed keys; `keyd-rust list-keys` lists all names. `#` starts a comment line.
The last binding wins.

| Binding | What it does |
| --- | --- |
| `a = b` | Map A to B. |
| `a = C-c` | Send Ctrl+C. |
| `j+k = esc` | J and K together produce Escape. |
| `f1 = macro(hello space world enter)` | Type "hello world", then Enter. |
| `rightcontrol = layer(arrows)` | Hold the `arrows` layer. |
| `space = overload(shift, space)` | Tap Space or hold Shift. |
| `f2 = overloadd(arrows, tab, esc, 250)` | Tap Tab, double tap Escape or hold `arrows`. |
| `a = noop` | Disable A. |

Prefixes are `C` Control, `A` Alt, `S` Shift, `M` Meta/Super and `G` AltGr.
Combine them as `C-S-z`. Bind Ctrl+J with `j = ...` in `[control]`, not `C-j = ...`.

For `overloadd`, choose a timeout in milliseconds. The second press must start
before the first release's deadline, then finish within its own timeout.
A lone tap waits; holding activates the layer immediately.

| Syntax | Purpose |
| --- | --- |
| `[ids]` | `*` matches all keyboards; ID selects one; `-ID` after `*` excludes it. |
| `[main]` / `[arrows]` | Default bindings / custom layer. Unmapped keys are unchanged. |
| `[arrows:C]` | Control on unbound keys; explicit bindings omit it. |
| `[control+shift]` | Both layers active. Define custom components first. |
| `oneshot(shift)` / `toggle(arrows)` | Layer for the next key / turn a layer on or off. |
| `clear()` | Clear toggled and oneshot layers. |
| `[name:layout]` / `setlayout(name)` | Defines / selects a layout. |
| `[global]` | Options, such as `chord_timeout = 50`. |
| `[aliases]` | Shared key names. |
| `include common` | Load from `/etc/keyd-rust/` or installed data, after `[ids]`. |

## Help and recovery

Hold **Backspace + Escape + Enter** to stop the daemon if the keyboard becomes
unusable. Fix the config and restart. Read errors with `sudo journalctl -eu keyd-rust`.

Only use trusted configs. `command(...)` runs as the daemon's user, usually root.
Treat `keyd-rust` group membership as root access.

Read the [manual](docs/keyd-rust.scdoc), or `man keyd-rust`, for all actions, options
and Unicode setup. More examples are in [examples](examples/) and the
[application mapper](docs/keyd-rust-application-mapper.scdoc) manual.

MIT license. See [LICENSE](LICENSE).
