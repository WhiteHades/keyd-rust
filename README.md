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

A Rust keyboard remapping daemon based on [keyd](https://github.com/rvaiya/keyd).

## Install

Use Rust 1.98 or newer and scdoc. On Arch Linux, build a pacman package:

```sh
bash packaging/build
sudo pacman -U .tmp/package/*.pkg.tar.zst
```

On other Linux systems, run `make && make man && sudo make install`.
Remove the original keyd package before starting the new service.

## Use

Put a configuration in `/etc/keyd-rust/`, then start the daemon:

```sh
sudo systemctl enable --now keyd-rust
sudo keyd-rust reload
```

See the [manual](docs/keyd-rust.scdoc) and [examples](examples/).

MIT license. See [LICENSE](LICENSE).
