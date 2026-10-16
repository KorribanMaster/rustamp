/* SPDX-License-Identifier: BSD-3-Clause */
/*
 * C OpenAMP peers for the Rust interop tests.
 *
 * Both peers run in the same process as the Rust side, on a thread, over a
 * shared buffer described to libmetal as one linear I/O region. They poll
 * the vrings with rproc_virtio_notified(..., RSC_NOTIFY_ID_ANY) instead of
 * waiting for interrupts.
 *
 * Shared-memory layout (offsets from the start of the buffer):
 *   0x00000  resource table: header, one vdev with two vrings
 *   0x01000  vring 0 (remote -> host)
 *   0x05000  vring 1 (host -> remote)
 *   0x10000  host buffer pool
 */
#include <sched.h>
#include <stdatomic.h>
#include <stdint.h>
#include <string.h>

#include <metal/io.h>
#include <metal/sys.h>
#include <openamp/open_amp.h>
#include <openamp/remoteproc_virtio.h>
#include <openamp/rpmsg_virtio.h>

#define VRING0_OFF   0x1000
#define VRING1_OFF   0x5000
#define POOL_OFF     0x10000
#define VRING_ALIGN  4096
#define SERVICE      "rpmsg-interop"
#define RPMSG_HDR_SIZE 16 /* struct rpmsg_hdr is private to open-amp */

METAL_PACKED_BEGIN
struct interop_rsc {
	uint32_t ver;
	uint32_t num;
	uint32_t reserved[2];
	uint32_t offset[1];
	struct fw_rsc_vdev vdev;
	struct fw_rsc_vdev_vring vring[2];
} METAL_PACKED_END;

struct peer {
	struct metal_io_region io;
	metal_phys_addr_t phys;
	struct virtio_device *vdev;
	struct rpmsg_virtio_device rvdev;
	struct rpmsg_endpoint ept;
	atomic_int bound;
	atomic_int quit;
	atomic_int kicks;
	int count;
	int errors;
	uint32_t expect_seq;
};

static int peer_notify(void *priv, uint32_t id)
{
	struct peer *p = priv;

	(void)id;
	atomic_fetch_add(&p->kicks, 1);
	return 0;
}

static void io_setup(struct peer *p, void *va, uint64_t pa, size_t size)
{
	p->phys = pa;
	/* page_shift >= address bits: one linear page covering the region. */
	metal_io_init(&p->io, va, &p->phys, size, (unsigned int)-1, 0, NULL);
}

static int vdev_setup(struct peer *p, unsigned int role)
{
	struct interop_rsc *rsc = metal_io_virt(&p->io, 0);
	unsigned int i;

	p->vdev = rproc_virtio_create_vdev(role, rsc->vdev.notifyid, &rsc->vdev,
					   &p->io, p, peer_notify, NULL);
	if (!p->vdev)
		return -1;
	for (i = 0; i < 2; i++) {
		void *va = metal_io_phys_to_virt(&p->io, rsc->vring[i].da);

		if (!va || rproc_virtio_init_vring(p->vdev, i,
						   rsc->vring[i].notifyid, va,
						   &p->io, rsc->vring[i].num,
						   rsc->vring[i].align))
			return -2;
	}
	return 0;
}

/* Expected message `seq`: 4-byte LE sequence number, then a pattern. */
static size_t make_msg(uint32_t seq, uint8_t *buf, size_t max)
{
	size_t len = 4 + (seq * 13) % (max - 4 + 1);
	size_t i;

	memcpy(buf, &seq, 4);
	for (i = 4; i < len; i++)
		buf[i] = (uint8_t)(seq + i);
	return len;
}

/* ---------------------------------------------------------------------
 * C remote (virtio device role): echoes every message on SERVICE.
 */

static struct peer remote;

static int remote_echo_cb(struct rpmsg_endpoint *ept, void *data, size_t len,
			  uint32_t src, void *priv)
{
	(void)priv;
	if (len == 4 && !memcmp(data, "quit", 4)) {
		atomic_store(&remote.quit, 1);
		return RPMSG_SUCCESS;
	}
	if (rpmsg_sendto(ept, data, len, src) < 0)
		remote.errors++;
	else
		remote.count++;
	return RPMSG_SUCCESS;
}

/* Writes the resource table the C remote's firmware would carry. */
void c_remote_write_table(void *va, uint64_t pa, uint32_t vring_num)
{
	struct interop_rsc *rsc = va;

	memset(rsc, 0, sizeof(*rsc));
	rsc->ver = 1;
	rsc->num = 1;
	rsc->offset[0] = offsetof(struct interop_rsc, vdev);
	rsc->vdev.type = RSC_VDEV;
	rsc->vdev.id = VIRTIO_ID_RPMSG;
	rsc->vdev.notifyid = 0;
	rsc->vdev.dfeatures = 1 << VIRTIO_RPMSG_F_NS;
	rsc->vdev.num_of_vrings = 2;
	rsc->vring[0].da = (uint32_t)(pa + VRING0_OFF);
	rsc->vring[0].align = VRING_ALIGN;
	rsc->vring[0].num = vring_num;
	rsc->vring[0].notifyid = 1;
	rsc->vring[1].da = (uint32_t)(pa + VRING1_OFF);
	rsc->vring[1].align = VRING_ALIGN;
	rsc->vring[1].num = vring_num;
	rsc->vring[1].notifyid = 2;
}

size_t c_rsc_table_size(void)
{
	return sizeof(struct interop_rsc);
}

/* Runs the C remote until it receives "quit"; returns messages echoed. */
int c_remote_run(void *va, uint64_t pa, size_t size)
{
	struct rpmsg_device *rdev;
	int ret;

	memset(&remote, 0, sizeof(remote));
	io_setup(&remote, va, pa, size);
	ret = vdev_setup(&remote, VIRTIO_DEV_DEVICE);
	if (ret)
		return -100 + ret;
	/* Blocks until the host sets DRIVER_OK. */
	ret = rpmsg_init_vdev(&remote.rvdev, remote.vdev, NULL, &remote.io,
			      NULL);
	if (ret)
		return -200;
	rdev = rpmsg_virtio_get_rpmsg_device(&remote.rvdev);
	ret = rpmsg_create_ept(&remote.ept, rdev, SERVICE, RPMSG_ADDR_ANY,
			       RPMSG_ADDR_ANY, remote_echo_cb, NULL);
	if (ret)
		return -300;
	while (!atomic_load(&remote.quit)) {
		rproc_virtio_notified(remote.vdev, RSC_NOTIFY_ID_ANY);
		sched_yield();
	}
	rpmsg_destroy_ept(&remote.ept);
	return remote.errors ? -400 - remote.errors : remote.count;
}

/* ---------------------------------------------------------------------
 * C host (virtio driver role): binds to SERVICE when the remote announces
 * it, sends numbered messages and checks the echoes in order.
 */

static struct peer host;

static int host_rx_cb(struct rpmsg_endpoint *ept, void *data, size_t len,
		      uint32_t src, void *priv)
{
	uint8_t exp[RPMSG_BUFFER_SIZE];
	size_t elen;

	(void)ept;
	(void)src;
	(void)priv;
	elen = make_msg(host.expect_seq, exp,
			RPMSG_BUFFER_SIZE - RPMSG_HDR_SIZE);
	if (len != elen || memcmp(data, exp, len))
		host.errors++;
	host.expect_seq++;
	host.count++;
	return RPMSG_SUCCESS;
}

static void host_ns_bind(struct rpmsg_device *rdev, const char *name,
			 uint32_t dest)
{
	if (strcmp(name, SERVICE))
		return;
	if (!rpmsg_create_ept(&host.ept, rdev, name, RPMSG_ADDR_ANY, dest,
			      host_rx_cb, NULL))
		atomic_store(&host.bound, 1);
}

/* Runs the C host for `n` round trips; returns verified echoes. */
int c_host_run(void *va, uint64_t pa, size_t size, uint32_t n)
{
	struct rpmsg_virtio_shm_pool pool;
	struct rpmsg_virtio_config cfg = {
		.h2r_buf_size = RPMSG_BUFFER_SIZE,
		.r2h_buf_size = RPMSG_BUFFER_SIZE,
		.split_shpool = false,
	};
	uint8_t buf[RPMSG_BUFFER_SIZE];
	uint32_t i;
	int ret;

	memset(&host, 0, sizeof(host));
	io_setup(&host, va, pa, size);
	ret = vdev_setup(&host, VIRTIO_DEV_DRIVER);
	if (ret)
		return -100 + ret;
	rpmsg_virtio_init_shm_pool(&pool, (uint8_t *)va + POOL_OFF,
				   size - POOL_OFF);
	ret = rpmsg_init_vdev_with_config(&host.rvdev, host.vdev, host_ns_bind,
					  &host.io, &pool, &cfg);
	if (ret)
		return -200;
	while (!atomic_load(&host.bound)) {
		rproc_virtio_notified(host.vdev, RSC_NOTIFY_ID_ANY);
		sched_yield();
	}
	for (i = 0; i < n; i++) {
		size_t len = make_msg(i, buf, sizeof(buf) - RPMSG_HDR_SIZE);

		if (rpmsg_send(&host.ept, buf, len) < 0)
			return -300;
		while (host.count <= (int)i) {
			rproc_virtio_notified(host.vdev, RSC_NOTIFY_ID_ANY);
			sched_yield();
		}
	}
	rpmsg_send(&host.ept, "quit", 4);
	return host.errors ? -400 - host.errors : host.count;
}

int c_peer_kicks(int which)
{
	return atomic_load(which ? &host.kicks : &remote.kicks);
}
