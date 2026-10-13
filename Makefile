.PHONY: all clean install uninstall debug man compose test test-io
VERSION=2.6.0
COMMIT=$(shell git describe --no-match --always --abbrev=7 --dirty 2>/dev/null || printf unknown)
VKBD=uinput
PREFIX?=/usr/local

CONFIG_DIR?=/etc/keyd
SOCKET_PATH=/var/run/keyd.socket
CARGO?=cargo
CARGO_TARGET_DIR?=target

quote = '$(subst ','"'"',$(1))'
BUILD_ENV = KEYD_BUILD_VERSION=$(call quote,v$(VERSION) ($(COMMIT))) \
	KEYD_PREFIX=$(call quote,$(PREFIX)) \
	KEYD_CONFIG_DIR=$(call quote,$(CONFIG_DIR)) \
	KEYD_DATA_DIR=$(call quote,$(PREFIX)/share/keyd) \
	KEYD_SOCKET_PATH=$(call quote,$(SOCKET_PATH)) \
	KEYD_VKBD=$(call quote,$(VKBD))

all:
	$(BUILD_ENV) $(CARGO) build --locked --release --target-dir $(call quote,$(CARGO_TARGET_DIR)) --bin keyd --bin keyd-application-mapper
	install -Dm755 $(call quote,$(CARGO_TARGET_DIR)/release/keyd) bin/keyd
	install -Dm755 $(call quote,$(CARGO_TARGET_DIR)/release/keyd-application-mapper) bin/keyd-application-mapper
debug:
	$(BUILD_ENV) $(CARGO) build --locked --target-dir $(call quote,$(CARGO_TARGET_DIR)) --bin keyd --bin keyd-application-mapper
	install -Dm755 $(call quote,$(CARGO_TARGET_DIR)/debug/keyd) bin/keyd
	install -Dm755 $(call quote,$(CARGO_TARGET_DIR)/debug/keyd-application-mapper) bin/keyd-application-mapper
compose:
	$(BUILD_ENV) $(CARGO) run --locked --release --target-dir $(call quote,$(CARGO_TARGET_DIR)) --bin generate_xcompose
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
		sed "s#@PREFIX@#$$service_prefix#g" keyd.service.in > keyd.service; \
		install -Dm644 keyd.service "$$prefix/lib/systemd/system/keyd.service"; \
		install -Dm644 data/sysusers.d "$$prefix/lib/sysusers.d/keyd.conf"; \
	else \
		echo "NOTE: systemd not found, you will need to manually add keyd to your system's init process."; \
	fi; \
	if [ $(call quote,$(VKBD)) = usb-gadget ]; then \
		sed "s#@PREFIX@#$$service_prefix#g" src/vkbd/usb-gadget.service.in > src/vkbd/usb-gadget.service; \
		install -Dm644 src/vkbd/usb-gadget.service "$$prefix/lib/systemd/system/keyd-usb-gadget.service"; \
		install -Dm755 src/vkbd/usb-gadget.sh "$$prefix/bin/keyd-usb-gadget.sh"; \
	fi; \
	mkdir -p "$$config" "$$prefix/bin" "$$prefix/share/keyd/layouts" \
		"$$prefix/share/man/man1" "$$prefix/share/doc/keyd/examples"; \
	if [ -z $(call quote,$(DESTDIR)) ]; then groupadd keyd || :; fi; \
	install -m755 bin/keyd bin/keyd-application-mapper "$$prefix/bin/"; \
	install -m644 docs/*.md README.md LICENSE "$$prefix/share/doc/keyd/"; \
	install -m644 examples/* "$$prefix/share/doc/keyd/examples/"; \
	install -m644 layouts/* "$$prefix/share/keyd/layouts/"; \
	cp -r data/gnome-* "$$prefix/share/keyd/"; \
	install -m644 data/*.1.gz "$$prefix/share/man/man1/"; \
	install -m644 data/keyd.compose "$$prefix/share/keyd/"
uninstall:
	@set -e; prefix=$(call quote,$(DESTDIR)$(PREFIX)); \
	if [ -z $(call quote,$(DESTDIR)) ]; then groupdel keyd || :; fi; \
	rm -f -- "$$prefix/bin/keyd" "$$prefix/bin/keyd-application-mapper" \
		"$$prefix/bin/keyd-usb-gadget.sh" \
		"$$prefix/share/keyd/keyd.compose" \
		"$$prefix/lib/systemd/system/keyd-usb-gadget.service" \
		"$$prefix/lib/systemd/system/keyd.service" \
		"$$prefix/lib/sysusers.d/keyd.conf"; \
	for f in docs/*.md README.md LICENSE; do rm -f -- "$$prefix/share/doc/keyd/$${f##*/}"; done; \
	for f in examples/*; do rm -f -- "$$prefix/share/doc/keyd/examples/$${f##*/}"; done; \
	for f in layouts/*; do rm -f -- "$$prefix/share/keyd/layouts/$${f##*/}"; done; \
	for f in data/*.1.gz; do rm -f -- "$$prefix/share/man/man1/$${f##*/}"; done; \
	for f in data/gnome-*/*; do rm -f -- "$$prefix/share/keyd/$${f#data/}"; done; \
	for d in data/gnome-*; do rmdir -- "$$prefix/share/keyd/$${d##*/}" 2>/dev/null || :; done; \
	for d in share/doc/keyd/examples share/doc/keyd share/keyd/layouts share/keyd; do \
		rmdir -- "$$prefix/$$d" 2>/dev/null || :; \
	done
clean:
	rm -f -- bin/keyd bin/keyd-application-mapper keyd.service src/vkbd/usb-gadget.service
	@rmdir bin 2>/dev/null || :
test: test-io
test-io:
	$(BUILD_ENV) $(CARGO) run --locked --target-dir $(call quote,$(CARGO_TARGET_DIR)) --example keyboard-e2e -- t/test.conf t/*.t
