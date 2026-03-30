TARGET = riscv64gc-unknown-linux-gnu
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

all:
	cd fuse_test && cargo build --release --target=$(TARGET) -Zbuild-std

install: all
	mkdir -p $(DISK_PATH)/musl
	cp fuse_test/target/$(TARGET)/release/fuse_test $(DISK_PATH)/musl/

clean:
	cd fuse_test && cargo clean
