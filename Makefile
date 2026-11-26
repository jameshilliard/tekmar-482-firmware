ELF := target/armv4t-none-eabi/release/tekmar-482-firmware
RAM_ELF := target/ram-trial/armv4t-none-eabi/release/tekmar-482-firmware
OBJCOPY ?= arm-none-eabi-objcopy

.PHONY: image ram-trial check
image:
	RUSTC_BOOTSTRAP=1 cargo firmware
	$(OBJCOPY) -O binary $(ELF) $(ELF).bin
	python3 tools/check_image.py $(ELF).bin
	$(OBJCOPY) -I binary -O ihex $(ELF).bin $(ELF).hex
	$(OBJCOPY) -I ihex -O binary $(ELF).hex $(ELF).roundtrip
	cmp $(ELF).bin $(ELF).roundtrip

ram-trial:
	RUSTC_BOOTSTRAP=1 cargo firmware --features ram-trial --target-dir target/ram-trial
	$(OBJCOPY) -O binary $(RAM_ELF) $(RAM_ELF).bin
	python3 tools/check_image.py --ram $(RAM_ELF).bin

check: image ram-trial
	cargo fmt --check
	cargo test --locked --lib
	RUSTC_BOOTSTRAP=1 cargo clippy --locked --release --target armv4t-none-eabi -Zbuild-std=core -- -D warnings
	uv run --script tests/host.py
	python3 tests/payload_test.py
	uv run --script tests/backup_test.py
	uv run --script tests/smoke.py $(ELF).bin
	uv run --script tests/smoke.py --ram $(RAM_ELF).bin
