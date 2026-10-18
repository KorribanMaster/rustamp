/* SPDX-License-Identifier: BSD-3-Clause */
/*
 * CPU0 entry for QEMU's xilinx-zynq-a9 without -kernel. With -kernel and
 * -smp 2, QEMU puts both its boot code and its secondary-CPU stub at
 * address 0 and refuses to start, so run.sh loads the kernel, DTB and
 * initramfs with -device loader and starts CPU0 here. This does what QEMU's
 * Zynq board setup and Linux boot code would: set the ARM PLL, then enter
 * the zImage with r0 = 0, r1 = ~0, r2 = DTB.
 *
 * Built with -DKERNEL=... -DDTB=... (see run.sh).
 */
	.arm
	.global _start
_start:
	ldr	r0, =0xf8000008		/* SLCR_UNLOCK */
	ldr	r1, =0xdf0d
	str	r1, [r0]
	ldr	r0, =0xf8000100		/* ARM_PLL_CTRL, as QEMU's board setup */
	ldr	r1, =0x00014008
	str	r1, [r0]
	ldr	r0, =0xf8000004		/* SLCR_LOCK */
	ldr	r1, =0x767b
	str	r1, [r0]
	mov	r0, #0
	mvn	r1, #0
	ldr	r2, =DTB
	ldr	r3, =KERNEL
	bx	r3
	.ltorg
