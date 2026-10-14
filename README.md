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

keyd-rust is a Linux keyboard remapping daemon written in Rust, based on
[keyd](https://github.com/rvaiya/keyd). It works in Wayland, X11 and the console.

## Differences from keyd

- The daemon and command-line tools are written in Rust, without a C engine or bridge.
- `overloadd` adds separate actions for a tap, double tap and hold. Its layer
  activates immediately, and the double tap has separate gap and release deadlines.

## Install

You need Rust 1.98 or newer, Cargo, make and scdoc. From a clean checkout,
build and install an Arch Linux package:

```sh
git clone https://github.com/WhiteHades/keyd-rust.git
cd keyd-rust
cargo fetch --locked
bash packaging/build
sudo pacman -U .tmp/package/*.pkg.tar.zst
```

On other Linux distributions, use `make && make man && sudo make install`.
The daemon needs access to `/dev/input` and `/dev/uinput`, and `getent` must
be installed. Stop and remove the original keyd before starting keyd-rust.
Keep a copy of your old configuration.

## Configure

Create `/etc/keyd-rust/default.conf`:

```sh
sudo install -d /etc/keyd-rust
sudoedit /etc/keyd-rust/default.conf
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

The example swaps Escape and Caps Lock. Hold Right Control with i/j/k/l
for arrow keys. Choose your own keys and layer names.

Check the file before starting the service:

```sh
sudo keyd-rust check
sudo systemctl enable --now keyd-rust
```

After editing a config, run `sudo keyd-rust check && sudo keyd-rust reload`.
Without systemd, run `keyd-rust` as root through your init system.
Existing keyd configs use the same syntax and can be copied into `/etc/keyd-rust/`.

## Syntax

A `.conf` file begins with `[ids]`. Use `*` for all keyboards, a device ID
for one keyboard, or `-0123:4567` after `*` to exclude a device.
Run `sudo keyd-rust monitor` to find device IDs and key names, or
`keyd-rust list-keys` to list every key name.

`[main]` contains the default bindings. Other sections define layers, which
are sets of bindings activated by a key. Unmapped keys keep their usual behavior.
Lines beginning with `#` are comments, and the last binding for a key wins.

| Binding | What it does |
| --- | --- |
| `a = b` | A produces B. |
| `a = C-c` | A produces Ctrl+C. |
| `j+k = esc` | J and K pressed together produce Escape. |
| `f1 = macro(hello space world enter)` | F1 types "hello world" and presses Enter. |
| `rightcontrol = layer(arrows)` | Right Control activates `arrows` while held. |
| `space = overload(shift, space)` | Space is Shift while held and Space when tapped. |
| `f2 = overloadd(arrows, tab, esc, 250)` | F2 is Tab on tap, Escape on double tap and `arrows` while held. |
| `a = noop` | A produces nothing. |

Modifier prefixes are `C` for Control, `A` for Alt, `S` for Shift,
`M` for Meta/Super and `G` for AltGr. Combine them with hyphens, as in `C-S-z`.
Use key names on the left, not modifier prefixes; put Ctrl+J bindings in `[control]`.

The final `overloadd` argument is a user-chosen timeout in milliseconds.
The second press must start before the first release's deadline and finish
within its own timeout. A lone tap waits for the timeout; the held layer
activates immediately.

`[arrows:C]` applies Control to unbound keys while `arrows` is active.
Explicit bindings omit that layer's modifier. `[control+shift]` defines
bindings used when both layers are active; define custom component layers first.

Use `oneshot(shift)` for the next key, `toggle(arrows)` to turn a layer on or off,
and `clear()` to clear toggled and oneshot layers. `setlayout(name)` selects
a layout defined by `[name:layout]`.

Put timing options such as `chord_timeout = 50` under `[global]`.
Use `[aliases]` to give keys shared names. `include common` loads a file from
`/etc/keyd-rust/` or the installed data directory; place it after `[ids]`.

## Help and recovery

Hold **Backspace + Escape + Enter** to terminate the daemon if a mapping makes
the keyboard unusable. Fix the config, then restart the service.
Read errors with `sudo journalctl -eu keyd-rust`.

Only use trusted configurations. `command(...)` runs shell commands as the
daemon's user, usually root. Treat membership in the `keyd-rust` group as root access.

The [manual](docs/keyd-rust.scdoc) covers every action, timing option, macros,
aliases, includes and Unicode setup. Read it locally with `man keyd-rust`.
See the [examples](examples/) and
[application mapper](docs/keyd-rust-application-mapper.scdoc) for more configurations.

MIT license. See [LICENSE](LICENSE).
