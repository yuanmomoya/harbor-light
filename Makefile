.PHONY: build icon package package-windows install uninstall test

CARGO ?= cargo

build:
	$(CARGO) build --release

test:
	$(CARGO) test

icon:
	chmod +x scripts/make-icon.sh
	./scripts/make-icon.sh

package: build
	chmod +x scripts/package.sh
	./scripts/package.sh

# Run this target from PowerShell on Windows. ARCH=x64 or ARCH=arm64.
package-windows:
	pwsh -File scripts/package-windows.ps1 -Architecture $(or $(ARCH),x64) -RequireInstaller

install: build
	chmod +x scripts/install.sh
	./scripts/install.sh

uninstall:
	chmod +x scripts/uninstall.sh
	./scripts/uninstall.sh
