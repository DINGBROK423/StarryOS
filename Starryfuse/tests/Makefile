TARGET = riscv64gc-unknown-linux-gnu
ARCH   = riscv64

ifneq ($(ARCH),)
  ifeq ($(ARCH), x86_64)
    TARGET := x86_64-unknown-linux-gnu
  else ifeq ($(ARCH), aarch64)
    TARGET := aarch64-unknown-linux-gnu
  else ifeq ($(ARCH), riscv64)
    TARGET := riscv64gc-unknown-linux-gnu
  else ifeq ($(ARCH), loongarch64)
    TARGET := loongarch64-unknown-linux-gnu
  endif
endif

DISK_PATH ?= $(abspath ../../disk)
IMG_PATH  ?= $(abspath ../../arceos/disk_$(ARCH).img)
MOUNT_DIR ?= $(abspath ../../.starryfuse_disk_mount)

TESTS := fuse_test fuse_rw_test fuse_mem_test

.PHONY: all install disk clean help

all:
	@for t in $(TESTS); do \
		echo "  [BUILD] $$t"; \
		cd $$t && cargo build --release --target=$(TARGET) -Zbuild-std || exit 1; \
		cd ..; \
	done

install: all
	@mkdir -p $(DISK_PATH)/musl
	@for t in $(TESTS); do \
		echo "  [INSTALL] $$t -> $(DISK_PATH)/musl/"; \
		cp -f $$t/target/$(TARGET)/release/$$t $(DISK_PATH)/musl/; \
	done

disk: all
	@if [ -f $(IMG_PATH) ]; then \
		echo "  [DISK] Updating image $(IMG_PATH) ..."; \
		mkdir -p $(MOUNT_DIR); \
		if sudo mount -o loop $(IMG_PATH) $(MOUNT_DIR); then \
			mkdir -p $(MOUNT_DIR)/musl; \
			for t in $(TESTS); do \
				echo "  [INSTALL] $$t -> $(MOUNT_DIR)/musl/"; \
				sudo cp -f $$t/target/$(TARGET)/release/$$t $(MOUNT_DIR)/musl/; \
			done; \
			sudo mkdir -p $(MOUNT_DIR)/root/modules; \
			for rlib in $(abspath ../../target)/riscv64gc-unknown-none-elf/release/deps/libstarryfuse-*.rlib; do \
				if [ -f "$$rlib" ]; then \
					echo "  [INSTALL] $$rlib -> $(MOUNT_DIR)/root/modules/"; \
					sudo cp -f "$$rlib" $(MOUNT_DIR)/root/modules/; \
				fi; \
			done; \
			sync; \
			sudo umount $(MOUNT_DIR) && rmdir $(MOUNT_DIR); \
			echo "  [DISK] Image updated."; \
		else \
			echo "  [ERROR] Failed to mount $(IMG_PATH)."; \
			rmdir $(MOUNT_DIR) 2>/dev/null; \
			exit 1; \
		fi \
	else \
		echo "  [WARNING] No disk image found at $(IMG_PATH)."; \
		echo "            Falling back to local disk directory ($(DISK_PATH))."; \
		echo "            If you use a pre-built rootfs image, run 'make disk' after"; \
		echo "            the image has been generated (e.g., after 'make img')."; \
		$(MAKE) install; \
	fi

clean:
	@for t in $(TESTS); do \
		echo "  [CLEAN] $$t"; \
		cd $$t && cargo clean; \
		cd ..; \
	done

help:
	@echo "Available targets:"
	@echo "  all      - Build all three test binaries (fuse_test, fuse_rw_test, fuse_mem_test)"
	@echo "  install  - Copy binaries to DISK_PATH/musl/ (default: $(DISK_PATH))"
	@echo "  disk     - Mount the rootfs image, install binaries, and unmount"
	@echo "             (ensures the QEMU disk image actually contains the latest tests)"
	@echo "  clean    - Run cargo clean for all test crates"
	@echo ""
	@echo "Variables:"
	@echo "  ARCH=$(ARCH)"
	@echo "  TARGET=$(TARGET)"
	@echo "  DISK_PATH=$(DISK_PATH)"
	@echo "  IMG_PATH=$(IMG_PATH)"
