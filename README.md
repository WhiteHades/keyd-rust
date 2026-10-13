# keyd-rust

keyd-rust is a maintained Rust rewrite of [keyd](https://github.com/rvaiya/keyd)
for Linux. The goal is a small, fast, memory-efficient keyboard remapping daemon
with simple controls and direct access to evdev and uinput.

## Design

- Use pure Rust, without a C engine, C bridge, or heavyweight runtime.
- Preserve keyd's configuration syntax, commands, and keyboard behavior.
- Keep the input loop small and avoid unnecessary allocation or background work.
- Measure latency, CPU use, and memory before making performance claims.

Existing behavior comes first. Optimizations and new controls must pass the same
compatibility checks, including interruption, release, and failure handling.

## Configuration

The compatibility contract includes device-specific configurations, layers,
oneshot modifiers, tap/hold actions, chords, macros, Unicode input, and dynamic
bindings through IPC. Configuration remains under `/etc/keyd/`.

```ini
[ids]
*

[main]
# Escape on tap, Control while held.
capslock = overload(control, esc)
```

Use the [configuration and command reference](docs/keyd.scdoc) and
[example configurations](examples/). The emergency
`Backspace+Escape+Enter` sequence remains part of the keyboard behavior contract.

## License

MIT. This project preserves keyd's original attribution and license in
[LICENSE](LICENSE).
