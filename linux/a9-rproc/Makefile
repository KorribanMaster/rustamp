# SPDX-License-Identifier: GPL-2.0-only
# Out-of-tree build:  make KDIR=/path/to/linux-build [ARCH=arm LLVM=1]
KDIR ?= /lib/modules/$(shell uname -r)/build

all:
	$(MAKE) -C $(KDIR) M=$(CURDIR) modules

clean:
	$(MAKE) -C $(KDIR) M=$(CURDIR) clean
