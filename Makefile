.PHONY: all clean install uninstall debug man compose test test-io
VERSION=2.6.0
COMMIT=$(shell git describe --no-match --always --abbrev=7 --dirty 2>/dev/null || printf unknown)
VKBD=uinput
PREFIX?=/usr/local

CONFIG_DIR?=/etc/keyd-rust
SOCKET_PATH=/var/run/keyd-rust.socket
CARGO?=cargo
CARGO_TARGET_DIR?=target
BUILD_DIR=$(CARGO_TARGET_DIR)$(if $(CARGO_BUILD_TARGET),/$(CARGO_BUILD_TARGET))
BINARIES=keyd-rust keyd-rust-application-mapper keyd-rust-generate-xcompose keyd-rust-dump-xkb-config
DOCS=README.md LICENSE

quote = '$(subst ','"'"',$(1))'
BUILD_ENV = $(if $(CARGO_BUILD_TARGET),CARGO_BUILD_TARGET=$(call quote,$(CARGO_BUILD_TARGET))) \
	KEYD_BUILD_VERSION=$(call quote,v$(VERSION) ($(COMMIT))) \
	KEYD_PREFIX=$(call quote,$(PREFIX)) \
	KEYD_CONFIG_DIR=$(call quote,$(CONFIG_DIR)) \
	KEYD_DATA_DIR=$(call quote,$(PREFIX)/share/keyd-rust) \
	KEYD_SOCKET_PATH=$(call quote,$(SOCKET_PATH)) \
	KEYD_VKBD=$(call quote,$(VKBD))

all:
	$(BUILD_ENV) $(CARGO) build --locked --release --target-dir $(call quote,$(CARGO_TARGET_DIR)) $(foreach binary,$(BINARIES),--bin $(binary))
	@set -e; for binary in $(BINARIES); do install -Dm755 $(call quote,$(BUILD_DIR)/release)/"$$binary" "bin/$$binary"; done
debug:
	$(BUILD_ENV) $(CARGO) build --locked --target-dir $(call quote,$(CARGO_TARGET_DIR)) $(foreach binary,$(BINARIES),--bin $(binary))
	@set -e; for binary in $(BINARIES); do install -Dm755 $(call quote,$(BUILD_DIR)/debug)/"$$binary" "bin/$$binary"; done
compose:
	$(BUILD_ENV) $(CARGO) run --locked --release --target-dir $(call quote,$(CARGO_TARGET_DIR)) --bin keyd-rust-generate-xcompose
man:
	mkdir -p data
	@set -e; for f in docs/*.scdoc; do \
		target=$${f%%.scdoc}.1.gz; \
		target=data/$${target##*/}; \
		trap 'rm -f "$$target.tmp" "$$target.gz.tmp"' EXIT HUP INT TERM; \
		scdoc < "$$f" > "$$target.tmp"; \
		gzip -c "$$target.tmp" > "$$target.gz.tmp"; \
		mv "$$target.gz.tmp" "$$target"; \
		rm -f "$$target.tmp"; \
	done
install: all
	@set -e; prefix=$(call quote,$(DESTDIR)$(PREFIX)); \
	config=$(call quote,$(DESTDIR)$(CONFIG_DIR)); \
	service_prefix=$$(printf '%s' $(call quote,$(PREFIX)) | sed 's/[\\\&#]/\\&/g'); \
	if [ -e /run/systemd/system ] || [ -n $(call quote,$(FORCE_SYSTEMD)) ]; then \
		sed "s#@PREFIX@#$$service_prefix#g" keyd-rust.service.in > keyd-rust.service; \
		install -Dm644 keyd-rust.service "$$prefix/lib/systemd/system/keyd-rust.service"; \
		install -Dm644 data/sysusers.d "$$prefix/lib/sysusers.d/keyd-rust.conf"; \
	else \
		echo "NOTE: systemd not found, you will need to manually add keyd-rust to your system's init process."; \
	fi; \
	if [ $(call quote,$(VKBD)) = usb-gadget ]; then \
		sed "s#@PREFIX@#$$service_prefix#g" src/vkbd/keyd-rust-usb-gadget.service.in > src/vkbd/keyd-rust-usb-gadget.service; \
		install -Dm644 src/vkbd/keyd-rust-usb-gadget.service "$$prefix/lib/systemd/system/keyd-rust-usb-gadget.service"; \
		install -Dm755 src/vkbd/keyd-rust-usb-gadget.sh "$$prefix/bin/keyd-rust-usb-gadget.sh"; \
	fi; \
	mkdir -p "$$config" "$$prefix/bin" "$$prefix/share/keyd-rust/layouts" \
		"$$prefix/share/man/man1" "$$prefix/share/doc/keyd-rust/examples"; \
	if [ -z $(call quote,$(DESTDIR)) ]; then groupadd keyd-rust || :; fi; \
	install -m755 $(addprefix bin/,$(BINARIES)) "$$prefix/bin/"; \
	install -m644 $(DOCS) "$$prefix/share/doc/keyd-rust/"; \
	install -m644 examples/* "$$prefix/share/doc/keyd-rust/examples/"; \
	install -m644 layouts/* "$$prefix/share/keyd-rust/layouts/"; \
	cp -r data/gnome-* "$$prefix/share/keyd-rust/"; \
	install -m644 data/*.1.gz "$$prefix/share/man/man1/"; \
	install -m644 data/keyd-rust.compose "$$prefix/share/keyd-rust/"
uninstall:
	@set -e; prefix=$(call quote,$(DESTDIR)$(PREFIX)); \
	if [ -z $(call quote,$(DESTDIR)) ]; then groupdel keyd-rust || :; fi; \
	rm -f -- "$$prefix/bin/keyd-rust" "$$prefix/bin/keyd-rust-application-mapper" \
		"$$prefix/bin/keyd-rust-usb-gadget.sh" \
		"$$prefix/bin/keyd-rust-generate-xcompose" \
		"$$prefix/bin/keyd-rust-dump-xkb-config" \
		"$$prefix/share/keyd-rust/keyd-rust.compose" \
		"$$prefix/lib/systemd/system/keyd-rust-usb-gadget.service" \
		"$$prefix/lib/systemd/system/keyd-rust.service" \
		"$$prefix/lib/sysusers.d/keyd-rust.conf"; \
	for f in $(DOCS); do rm -f -- "$$prefix/share/doc/keyd-rust/$${f##*/}"; done; \
	for f in examples/*; do rm -f -- "$$prefix/share/doc/keyd-rust/examples/$${f##*/}"; done; \
	for f in layouts/*; do rm -f -- "$$prefix/share/keyd-rust/layouts/$${f##*/}"; done; \
	for f in data/*.1.gz; do rm -f -- "$$prefix/share/man/man1/$${f##*/}"; done; \
	for f in data/gnome-*/*; do rm -f -- "$$prefix/share/keyd-rust/$${f#data/}"; done; \
	for d in data/gnome-*; do rmdir -- "$$prefix/share/keyd-rust/$${d##*/}" 2>/dev/null || :; done; \
	for d in share/doc/keyd-rust/examples share/doc/keyd-rust share/keyd-rust/layouts share/keyd-rust; do \
		rmdir -- "$$prefix/$$d" 2>/dev/null || :; \
	done
clean:
	rm -f -- $(addprefix bin/,$(BINARIES)) keyd-rust.service src/vkbd/keyd-rust-usb-gadget.service
	@rmdir bin 2>/dev/null || :
test: test-io
test-io:
	$(BUILD_ENV) $(CARGO) run --locked --target-dir $(call quote,$(CARGO_TARGET_DIR)) --example keyboard-e2e -- t/test.conf t/*.t
	$(BUILD_ENV) $(CARGO) run --locked --target-dir $(call quote,$(CARGO_TARGET_DIR)) --example double-tap-e2e -- t/double-tap.conf
