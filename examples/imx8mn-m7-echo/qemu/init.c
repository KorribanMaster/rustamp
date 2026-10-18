/* SPDX-License-Identifier: BSD-3-Clause */
/*
 * /init for the QEMU rpmsg tests: no libc, raw syscalls (aarch64 for the
 * imx8mp-evk test, 32-bit ARM EABI for the Cortex-A9 test).
 *
 *  1. mounts devtmpfs, sysfs, proc
 *  2. starts the M7 firmware through remoteproc's sysfs interface
 *  3. waits for /dev/ttyRPMSG0 (rpmsg_tty bound to the firmware's
 *     "rpmsg-tty" announcement)
 *  4. sends checked messages and compares the echo byte for byte
 *  5. with -DRELOAD_CYCLES=N: stops the remote, starts it again and
 *     repeats the echo test, N times
 *  6. prints a verdict line and powers off
 *
 * Build: clang --target=aarch64-linux-gnu -ffreestanding -nostdlib -static
 *        -fno-stack-protector -O2 -o init init.c
 *   (or --target=armv7a-linux-gnueabi)
 */
typedef unsigned long u64;
typedef long i64;
typedef unsigned int u32;
typedef unsigned char u8;

#ifndef RELOAD_CYCLES
#define RELOAD_CYCLES 0
#endif

#if defined(__aarch64__)
#define SYS_mkdirat 34
#define SYS_mount 40
#define SYS_openat 56
#define SYS_close 57
#define SYS_read 63
#define SYS_write 64
#define SYS_ppoll 73
#define SYS_ioctl 29
#define SYS_nanosleep 101
#define SYS_reboot 142
#elif defined(__arm__)
#define SYS_mkdirat 323
#define SYS_mount 21
#define SYS_openat 322
#define SYS_close 6
#define SYS_read 3
#define SYS_write 4
#define SYS_ppoll 336
#define SYS_ioctl 54
#define SYS_nanosleep 162
#define SYS_reboot 88
#else
#error "aarch64 or 32-bit ARM only"
#endif
#define AT_FDCWD (-100)
#define O_RDONLY 0
#define O_WRONLY 1
#define O_RDWR 2
#define O_NOCTTY 0400
#define O_NONBLOCK 04000

static i64 sys6(i64 n, i64 a, i64 b, i64 c, i64 d, i64 e, i64 f)
{
#if defined(__arm__)
	register i64 r7 __asm__("r7") = n;
	register i64 r0 __asm__("r0") = a;
	register i64 r1 __asm__("r1") = b;
	register i64 r2 __asm__("r2") = c;
	register i64 r3 __asm__("r3") = d;
	register i64 r4 __asm__("r4") = e;
	register i64 r5 __asm__("r5") = f;
	__asm__ volatile("svc #0"
			 : "+r"(r0)
			 : "r"(r7), "r"(r1), "r"(r2), "r"(r3), "r"(r4), "r"(r5)
			 : "memory");
	return r0;
#else
	register i64 x8 __asm__("x8") = n;
	register i64 x0 __asm__("x0") = a;
	register i64 x1 __asm__("x1") = b;
	register i64 x2 __asm__("x2") = c;
	register i64 x3 __asm__("x3") = d;
	register i64 x4 __asm__("x4") = e;
	register i64 x5 __asm__("x5") = f;
	__asm__ volatile("svc #0"
			 : "+r"(x0)
			 : "r"(x8), "r"(x1), "r"(x2), "r"(x3), "r"(x4), "r"(x5)
			 : "memory");
	return x0;
#endif
}
#define sys(n, a, b, c) sys6(n, (i64)(a), (i64)(b), (i64)(c), 0, 0, 0)

static u64 slen(const char *s) { u64 n = 0; while (s[n]) n++; return n; }
static void out(const char *s) { sys(SYS_write, 1, s, slen(s)); }

static void outnum(i64 v)
{
	char b[24];
	int i = 23;
	int neg = v < 0;
	u64 u = neg ? -v : v;

	b[i] = 0;
	do { b[--i] = '0' + u % 10; u /= 10; } while (u);
	if (neg)
		b[--i] = '-';
	out(b + i);
}

static void msleep(i64 ms)
{
	i64 ts[2] = { ms / 1000, (ms % 1000) * 1000000 };
	sys(SYS_nanosleep, ts, 0, 0);
}

static int write_file(const char *path, const char *val)
{
	i64 fd = sys(SYS_openat, AT_FDCWD, path, O_WRONLY);
	i64 r;

	if (fd < 0)
		return (int)fd;
	r = sys(SYS_write, fd, val, slen(val));
	sys(SYS_close, fd, 0, 0);
	return r < 0 ? (int)r : 0;
}

static void cat(const char *path)
{
	char b[256];
	i64 fd = sys(SYS_openat, AT_FDCWD, path, O_RDONLY);
	i64 n;

	if (fd < 0)
		return;
	n = sys(SYS_read, fd, b, sizeof(b) - 1);
	if (n > 0) {
		b[n] = 0;
		out(b);
	}
	sys(SYS_close, fd, 0, 0);
}

/* struct termios (the same layout on aarch64 and 32-bit ARM) */
struct termios {
	u32 c_iflag, c_oflag, c_cflag, c_lflag;
	u8 c_line;
	u8 c_cc[19];
};
#define TCGETS 0x5401
#define TCSETS 0x5402

static void make_raw(i64 fd)
{
	struct termios t;

	if (sys(SYS_ioctl, fd, TCGETS, &t) < 0)
		return;
	t.c_iflag &= ~(0000001 | 0000002 | 0000010 | 0000040 | 0000100 |
		       0000200 | 0000400 | 0002000); /* IGNBRK..IXON */
	t.c_oflag &= ~0000001;                        /* OPOST */
	t.c_lflag &= ~(0000010 | 0000100 | 0000002 | 0000001 | 0100000);
	t.c_cflag &= ~(0000060 | 0000400);            /* CSIZE, PARENB */
	t.c_cflag |= 0000060;                         /* CS8 */
	t.c_cc[6] = 1;                                /* VMIN */
	t.c_cc[5] = 0;                                /* VTIME */
	sys(SYS_ioctl, fd, TCSETS, &t);
}

struct pollfd { int fd; short events, revents; };

/* Reads exactly n bytes or times out (returns bytes read). */
static u64 read_n(i64 fd, u8 *buf, u64 n, int timeout_ms)
{
	u64 got = 0;

	while (got < n) {
		struct pollfd p = { (int)fd, 1 /* POLLIN */, 0 };
		i64 ts[2] = { timeout_ms / 1000, (timeout_ms % 1000) * 1000000 };
		i64 r = sys6(SYS_ppoll, (i64)&p, 1, (i64)ts, 0, 0, 0);

		if (r <= 0)
			break;
		r = sys(SYS_read, fd, buf + got, n - got);
		if (r <= 0)
			break;
		got += r;
	}
	return got;
}

static u8 pattern(u32 seq, u32 i) { return (u8)(seq * 31 + i * 7 + (i >> 3)); }

#define ROUNDS 2000
#define MAXLEN 256

/* Starts the remote and runs the echo test; returns the bytes echoed or 0. */
static u64 echo_test(void)
{
	static u8 tx[MAXLEN], rx[MAXLEN];
	i64 fd = -1;
	u64 total = 0;
	u32 seq, i;
	int t;

	if (write_file("/sys/class/remoteproc/remoteproc0/state", "start")) {
		out("[init] FAIL: could not start remoteproc0\n");
		return 0;
	}
	out("[init] remoteproc0 state: ");
	cat("/sys/class/remoteproc/remoteproc0/state");

	for (t = 0; t < 600 && fd < 0; t++) {
		fd = sys(SYS_openat, AT_FDCWD, "/dev/ttyRPMSG0",
			 O_RDWR | O_NOCTTY);
		if (fd < 0)
			msleep(100);
	}
	if (fd < 0) {
		out("[init] FAIL: /dev/ttyRPMSG0 did not appear\n");
		return 0;
	}
	out("[init] /dev/ttyRPMSG0 is up after ");
	outnum(t * 100);
	out(" ms\n");
	make_raw(fd);

	for (seq = 0; seq < ROUNDS; seq++) {
		u32 len = 1 + (seq * 37) % MAXLEN;

		for (i = 0; i < len; i++)
			tx[i] = pattern(seq, i);
		if (sys(SYS_write, fd, tx, len) != (i64)len) {
			out("[init] FAIL: short write at round ");
			outnum(seq);
			out("\n");
			return 0;
		}
		if (read_n(fd, rx, len, 5000) != len) {
			out("[init] FAIL: echo timeout at round ");
			outnum(seq);
			out("\n");
			return 0;
		}
		for (i = 0; i < len; i++) {
			if (rx[i] != tx[i]) {
				out("[init] FAIL: mismatch at round ");
				outnum(seq);
				out(" byte ");
				outnum(i);
				out("\n");
				return 0;
			}
		}
		total += len;
	}
	sys(SYS_close, fd, 0, 0);
	return total;
}

static int run(void)
{
	u64 total;
	int cycle;

	out("\n[init] RustAmp QEMU rpmsg test\n");
	out("[init] remoteproc0 state: ");
	cat("/sys/class/remoteproc/remoteproc0/state");
	if (write_file("/sys/class/remoteproc/remoteproc0/firmware",
		       "rpmsg-echo.elf")) {
		out("[init] FAIL: could not set the firmware name\n");
		return 1;
	}
	total = echo_test();
	if (!total)
		return 1;
	out("[init] ");
	outnum(ROUNDS);
	out(" round trips, ");
	outnum(total);
	out(" bytes echoed\n");
	for (cycle = 1; cycle <= RELOAD_CYCLES; cycle++) {
		u64 n;

		if (write_file("/sys/class/remoteproc/remoteproc0/state", "stop")) {
			out("[init] FAIL: could not stop remoteproc0\n");
			return 1;
		}
		out("[init] stopped; remoteproc0 state: ");
		cat("/sys/class/remoteproc/remoteproc0/state");
		n = echo_test();
		if (!n)
			return 1;
		out("[init] reload ");
		outnum(cycle);
		out(": ");
		outnum(ROUNDS);
		out(" round trips, ");
		outnum(n);
		out(" bytes echoed\n");
		total += n;
	}
	out("[init] PASS: ");
	outnum((RELOAD_CYCLES + 1) * ROUNDS);
	out(" round trips, ");
	outnum(total);
	out(" bytes echoed by the RustAmp firmware");
	if (RELOAD_CYCLES) {
		out(" across ");
		outnum(RELOAD_CYCLES);
		out(" stop/start reloads");
	}
	out("\n");
	return 0;
}

void _start(void)
{
	sys(SYS_mkdirat, AT_FDCWD, "/dev", 0755);
	sys(SYS_mkdirat, AT_FDCWD, "/sys", 0755);
	sys(SYS_mkdirat, AT_FDCWD, "/proc", 0755);
	sys6(SYS_mount, (i64)"devtmpfs", (i64)"/dev", (i64)"devtmpfs", 0, 0, 0);
	sys6(SYS_mount, (i64)"sysfs", (i64)"/sys", (i64)"sysfs", 0, 0, 0);
	sys6(SYS_mount, (i64)"proc", (i64)"/proc", (i64)"proc", 0, 0, 0);
	{
		i64 c = sys(SYS_openat, AT_FDCWD, "/dev/console", O_RDWR);

		if (c >= 0 && c != 1) {
			/* fds 0..2 are not open in a bare initramfs init */
			sys(SYS_openat, AT_FDCWD, "/dev/console", O_RDWR);
			sys(SYS_openat, AT_FDCWD, "/dev/console", O_RDWR);
		}
	}
	run();
	msleep(500);
#ifdef EXIT_BY_RESTART
	/* No power-off on this machine: restart, which -no-reboot turns into
	 * a QEMU exit. LINUX_REBOOT_CMD_RESTART */
	out("[init] restarting (QEMU exits)\n");
	sys6(SYS_reboot, 0xfee1dead, 672274793, 0x01234567, 0, 0, 0);
#else
	out("[init] powering off\n");
	/* LINUX_REBOOT_MAGIC1/2, LINUX_REBOOT_CMD_POWER_OFF */
	sys6(SYS_reboot, 0xfee1dead, 672274793, 0x4321fedc, 0, 0, 0);
#endif
	for (;;)
		;
}

/* The compiler may still emit these for struct initialisers. */
void *memset(void *d, int c, u64 n)
{
	u8 *p = d;

	while (n--)
		*p++ = (u8)c;
	return d;
}

void *memcpy(void *d, const void *s, u64 n)
{
	u8 *p = d;
	const u8 *q = s;

	while (n--)
		*p++ = *q++;
	return d;
}
