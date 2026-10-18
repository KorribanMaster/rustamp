// SPDX-License-Identifier: GPL-2.0-only
/*
 * Remoteproc driver for a spare Cortex-A9 core (Linux on CPU0, an RTU on
 * CPU1 of the same MPCore cluster): Zynq-7000, Arria 10, or any A9 with a
 * way to release the second core.
 *
 * Notifications are SGIs in both directions, as in the original OpenAMP
 * Zynq-7000 port: Linux raises tx-sgi on the RTU's core by writing
 * GICD_SGIR, and the RTU raises rx-sgi on Linux's core. Linux's GIC driver
 * only allocates SGIs 0-7 for IPIs, so 8-15 are free; rx-sgi is mapped with
 * a one-cell fwspec, which gic_irq_domain_translate() takes as an SGI.
 *
 * Starting:
 *   - First start: the "release-addr" method writes the entry address to a
 *     mailbox word the parked core polls (the Zynq boot ROM's 0xfffffff0
 *     after the FSBL; address 0 for QEMU's boot stub), then wakes it with
 *     SEV and the SGI. "socfpga-a10" takes CPU1 out of reset through the
 *     Arria 10 reset manager, like socfpga_a10_boot_secondary().
 *   - Restarts: stopping is cooperative. Linux sets the park page's stop
 *     request and kicks; the firmware parks in a stub in that page and
 *     writes PARK_MAGIC. The next start writes the new entry to the page's
 *     release word. See examples/cortex-a9-rtu-echo/src/park.rs.
 *
 * Linux must not use the RTU's core: boot with maxcpus=1 (or fewer CPUs
 * than the RTU's index).
 */

#include <linux/cpu.h>
#include <linux/delay.h>
#include <linux/interrupt.h>
#include <linux/io.h>
#include <linux/irq.h>
#include <linux/irqdomain.h>
#include <linux/module.h>
#include <linux/of.h>
#include <linux/of_address.h>
#include <linux/of_irq.h>
#include <linux/of_reserved_mem.h>
#include <linux/percpu.h>
#include <linux/platform_device.h>
#include <linux/remoteproc.h>
#include <linux/workqueue.h>

#include <asm/cacheflush.h>

#if __has_include("remoteproc_internal.h")
#include "remoteproc_internal.h"
#else
/* Exported by the remoteproc core but declared in its private header. */
irqreturn_t rproc_vq_interrupt(struct rproc *rproc, int vq_id);
int rproc_elf_sanity_check(struct rproc *rproc, const struct firmware *fw);
u64 rproc_elf_get_boot_addr(struct rproc *rproc, const struct firmware *fw);
int rproc_elf_load_segments(struct rproc *rproc, const struct firmware *fw);
int rproc_elf_load_rsc_table(struct rproc *rproc, const struct firmware *fw);
struct resource_table *rproc_elf_find_loaded_rsc_table(struct rproc *rproc,
							const struct firmware *fw);
#endif

#define GICD_SGIR		0xf00

/* Park page protocol (park.rs). */
#define PARK_STOP_REQ		0xff0
#define PARK_MAGIC_OFF		0xff8
#define PARK_RELEASE		0xffc
#define PARK_MAGIC		0x4b524150	/* "PARK" */
#define PARK_TIMEOUT_MS		1000

/* Arria 10 reset manager. */
#define A10_RSTMGR_MPUMODRST	0x20
#define A10_MPUMODRST_CPU1	BIT(1)

enum a9_start_method {
	A9_START_RELEASE_ADDR,
	A9_START_SOCFPGA_A10,
};

struct a9_rproc {
	struct device *dev;
	struct rproc *rproc;
	unsigned int cpu;		/* RTU core (physical CPU id) */
	unsigned int host_cpu;		/* Linux CPU the RTU signals */
	u32 tx_sgi, rx_sgi;
	void __iomem *gicd;
	int rx_irq;
	struct a9_rproc * __percpu *pcpu;
	struct work_struct vq_work;
	u32 park_addr;
	enum a9_start_method method;
	phys_addr_t release_addr;	/* release-addr */
	void __iomem *rstmgr;		/* socfpga-a10 */
	void __iomem *cpu1start;	/* socfpga-a10 */
};

static void a9_rproc_sgi(struct a9_rproc *priv)
{
	/* Make ring updates visible before the RTU takes the interrupt. */
	wmb();
	writel_relaxed(BIT(16 + priv->cpu) | priv->tx_sgi, priv->gicd + GICD_SGIR);
}

static int a9_rproc_notified(int id, void *p, void *data)
{
	rproc_vq_interrupt(data, id);
	return 0;
}

static void a9_rproc_vq_work(struct work_struct *work)
{
	struct a9_rproc *priv = container_of(work, struct a9_rproc, vq_work);

	idr_for_each(&priv->rproc->notifyids, a9_rproc_notified, priv->rproc);
}

static irqreturn_t a9_rproc_isr(int irq, void *dev_id)
{
	struct a9_rproc *priv = *(struct a9_rproc **)dev_id;

	queue_work(system_highpri_wq, &priv->vq_work);
	return IRQ_HANDLED;
}

static void __iomem *a9_rproc_park_page(struct a9_rproc *priv)
{
	bool is_iomem = false;
	void *va = rproc_da_to_va(priv->rproc, priv->park_addr, SZ_4K, &is_iomem);

	if (!va || !is_iomem)
		return NULL;
	return (void __iomem *)va;
}

static int a9_rproc_mem_alloc(struct rproc *rproc, struct rproc_mem_entry *mem)
{
	mem->va = ioremap_wc(mem->dma, mem->len);
	if (!mem->va)
		return -ENOMEM;
	mem->is_iomem = true;
	return 0;
}

static int a9_rproc_mem_release(struct rproc *rproc, struct rproc_mem_entry *mem)
{
	iounmap(mem->va);
	return 0;
}

static int a9_rproc_prepare(struct rproc *rproc)
{
	struct a9_rproc *priv = rproc->priv;
	struct device_node *np = priv->dev->of_node;
	struct rproc_mem_entry *mem;
	struct resource res;
	int i;

	for (i = 0; !of_reserved_mem_region_to_resource(np, i, &res); i++) {
		int len = strchrnul(res.name, '@') - res.name;

		/* The vdev buffer pool is assigned by rproc_add_virtio_dev(). */
		if (strstarts(res.name, "vdev0buffer")) {
			mem = rproc_of_resm_mem_entry_init(priv->dev, i,
							   resource_size(&res),
							   res.start, "vdev0buffer");
		} else {
			/* The RTU sees physical addresses: da == pa. */
			mem = rproc_mem_entry_init(priv->dev, NULL, res.start,
						   resource_size(&res), res.start,
						   a9_rproc_mem_alloc,
						   a9_rproc_mem_release,
						   "%.*s", len, res.name);
		}
		if (!mem)
			return -ENOMEM;
		rproc_add_carveout(rproc, mem);
	}
	return 0;
}

static int a9_rproc_parse_fw(struct rproc *rproc, const struct firmware *fw)
{
	return rproc_elf_load_rsc_table(rproc, fw);
}

/*
 * Writes words at a physical address the RTU core reads with its caches
 * off. ARM Linux refuses ioremap() of RAM (QEMU's boot stub polls address
 * 0, and the A10 trampoline lives there too), so RAM goes through the
 * linear map plus a clean to the point of coherency.
 */
static int a9_rproc_write_phys(phys_addr_t pa, const u32 *val, unsigned int n)
{
	unsigned int i;

	if (pfn_valid(PHYS_PFN(pa))) {
		u32 *va = phys_to_virt(pa);

		for (i = 0; i < n; i++)
			WRITE_ONCE(va[i], val[i]);
		__sync_cache_range_w(va, n * sizeof(u32));
	} else {
		void __iomem *io = ioremap(pa, n * sizeof(u32));

		if (!io)
			return -ENOMEM;
		for (i = 0; i < n; i++)
			writel(val[i], io + 4 * i);
		iounmap(io);
	}
	return 0;
}

static int a9_rproc_release_addr_start(struct a9_rproc *priv, u32 entry)
{
	int ret = a9_rproc_write_phys(priv->release_addr, &entry, 1);

	if (ret)
		return ret;
	/* Wake a WFE loop (boot ROM) and a WFI loop (QEMU's boot stub). */
	dsb(ishst);
	sev();
	a9_rproc_sgi(priv);
	return 0;
}

static int a9_rproc_a10_start(struct a9_rproc *priv, u32 entry)
{
	/* The same sequence as socfpga_a10_boot_secondary(). */
	const u32 trampoline[2] = {
		0xe51ff004,	/* ldr pc, [pc, #-4] */
		entry,
	};
	int ret;

	writel(A10_MPUMODRST_CPU1, priv->rstmgr + A10_RSTMGR_MPUMODRST);
	ret = a9_rproc_write_phys(0, trampoline, ARRAY_SIZE(trampoline));
	if (ret)
		return ret;
	writel(entry, priv->cpu1start);
	/* Trampoline and start address must land before CPU1 leaves reset. */
	wmb();
	writel(0, priv->rstmgr + A10_RSTMGR_MPUMODRST);
	return 0;
}

static int a9_rproc_start(struct rproc *rproc)
{
	struct a9_rproc *priv = rproc->priv;
	void __iomem *park = a9_rproc_park_page(priv);
	u32 entry = rproc->bootaddr;

	if (cpu_online(priv->cpu)) {
		dev_err(priv->dev, "CPU%u is online in Linux; boot with maxcpus=%u\n",
			priv->cpu, priv->cpu);
		return -EBUSY;
	}

	if (park && readl(park + PARK_MAGIC_OFF) == PARK_MAGIC) {
		dev_dbg(priv->dev, "releasing parked CPU%u at %#x\n", priv->cpu, entry);
		writel(0, park + PARK_STOP_REQ);
		writel(entry, park + PARK_RELEASE);
		a9_rproc_sgi(priv);
		return 0;
	}
	if (park)
		writel(0, park + PARK_STOP_REQ);

	switch (priv->method) {
	case A9_START_SOCFPGA_A10:
		return a9_rproc_a10_start(priv, entry);
	case A9_START_RELEASE_ADDR:
	default:
		return a9_rproc_release_addr_start(priv, entry);
	}
}

static int a9_rproc_stop(struct rproc *rproc)
{
	struct a9_rproc *priv = rproc->priv;
	void __iomem *park = a9_rproc_park_page(priv);
	unsigned long timeout;

	if (!park) {
		dev_warn(priv->dev, "no park page at %#x; CPU%u keeps running\n",
			 priv->park_addr, priv->cpu);
		return 0;
	}
	writel(1, park + PARK_STOP_REQ);
	a9_rproc_sgi(priv);
	timeout = jiffies + msecs_to_jiffies(PARK_TIMEOUT_MS);
	while (readl(park + PARK_MAGIC_OFF) != PARK_MAGIC) {
		if (time_after(jiffies, timeout)) {
			if (priv->method == A9_START_SOCFPGA_A10) {
				dev_warn(priv->dev, "CPU%u did not park, resetting it\n",
					 priv->cpu);
				writel(A10_MPUMODRST_CPU1,
				       priv->rstmgr + A10_RSTMGR_MPUMODRST);
				return 0;
			}
			dev_err(priv->dev, "CPU%u did not park\n", priv->cpu);
			return -ETIMEDOUT;
		}
		usleep_range(1000, 2000);
	}
	dev_dbg(priv->dev, "CPU%u parked\n", priv->cpu);
	return 0;
}

static void a9_rproc_kick(struct rproc *rproc, int vqid)
{
	a9_rproc_sgi(rproc->priv);
}

static const struct rproc_ops a9_rproc_ops = {
	.prepare		= a9_rproc_prepare,
	.start			= a9_rproc_start,
	.stop			= a9_rproc_stop,
	.kick			= a9_rproc_kick,
	.load			= rproc_elf_load_segments,
	.parse_fw		= a9_rproc_parse_fw,
	.find_loaded_rsc_table	= rproc_elf_find_loaded_rsc_table,
	.sanity_check		= rproc_elf_sanity_check,
	.get_boot_addr		= rproc_elf_get_boot_addr,
};

static void a9_rproc_enable_irq(void *data)
{
	struct a9_rproc *priv = data;

	if (smp_processor_id() == priv->host_cpu)
		enable_percpu_irq(priv->rx_irq, IRQ_TYPE_NONE);
}

static void a9_rproc_disable_irq(void *data)
{
	struct a9_rproc *priv = data;

	if (smp_processor_id() == priv->host_cpu)
		disable_percpu_irq(priv->rx_irq);
}

static void a9_rproc_free_irq(void *data)
{
	struct a9_rproc *priv = data;

	on_each_cpu(a9_rproc_disable_irq, priv, 1);
	free_percpu_irq(priv->rx_irq, priv->pcpu);
	irq_dispose_mapping(priv->rx_irq);
	free_percpu(priv->pcpu);
}

static int a9_rproc_init_gic(struct a9_rproc *priv)
{
	struct device *dev = priv->dev;
	struct device_node *gic;
	struct irq_fwspec fwspec = { .param_count = 1 };
	int cpu, ret;

	gic = of_irq_find_parent(dev->of_node);
	if (!gic)
		return dev_err_probe(dev, -ENODEV, "no interrupt parent (GIC)\n");
	priv->gicd = devm_of_iomap(dev, gic, 0, NULL);
	if (IS_ERR(priv->gicd)) {
		/* The GIC driver owns the region; map it without claiming. */
		struct resource r;

		ret = of_address_to_resource(gic, 0, &r);
		priv->gicd = ret ? NULL : devm_ioremap(dev, r.start, resource_size(&r));
	}
	if (!priv->gicd) {
		of_node_put(gic);
		return dev_err_probe(dev, -ENOMEM, "cannot map the GIC distributor\n");
	}

	fwspec.fwnode = of_fwnode_handle(gic);
	fwspec.param[0] = priv->rx_sgi;
	priv->rx_irq = irq_create_fwspec_mapping(&fwspec);
	of_node_put(gic);
	if (priv->rx_irq <= 0)
		return dev_err_probe(dev, -EINVAL, "cannot map SGI %u\n", priv->rx_sgi);

	priv->pcpu = alloc_percpu(struct a9_rproc *);
	if (!priv->pcpu) {
		irq_dispose_mapping(priv->rx_irq);
		return -ENOMEM;
	}
	for_each_possible_cpu(cpu)
		*per_cpu_ptr(priv->pcpu, cpu) = priv;

	ret = request_percpu_irq(priv->rx_irq, a9_rproc_isr, dev_name(dev), priv->pcpu);
	if (ret) {
		free_percpu(priv->pcpu);
		irq_dispose_mapping(priv->rx_irq);
		return dev_err_probe(dev, ret, "cannot request SGI %u\n", priv->rx_sgi);
	}
	on_each_cpu(a9_rproc_enable_irq, priv, 1);
	return devm_add_action_or_reset(dev, a9_rproc_free_irq, priv);
}

static int a9_rproc_init_start_method(struct a9_rproc *priv)
{
	struct device *dev = priv->dev;
	struct device_node *np = dev->of_node;
	const char *method = "release-addr";
	u64 addr;

	of_property_read_string(np, "openamp-rs,start-method", &method);
	if (!strcmp(method, "release-addr")) {
		priv->method = A9_START_RELEASE_ADDR;
		if (of_property_read_u64(np, "openamp-rs,release-addr", &addr)) {
			u32 a32;

			if (of_property_read_u32(np, "openamp-rs,release-addr", &a32))
				return dev_err_probe(dev, -EINVAL,
						     "release-addr needs openamp-rs,release-addr\n");
			addr = a32;
		}
		priv->release_addr = addr;
		return 0;
	}
	if (!strcmp(method, "socfpga-a10")) {
		struct device_node *n;
		u32 start;

		priv->method = A9_START_SOCFPGA_A10;
		n = of_parse_phandle(np, "altr,rst-mgr", 0);
		if (!n)
			return dev_err_probe(dev, -EINVAL, "socfpga-a10 needs altr,rst-mgr\n");
		priv->rstmgr = devm_of_iomap(dev, n, 0, NULL);
		of_node_put(n);
		if (IS_ERR(priv->rstmgr))
			return dev_err_probe(dev, PTR_ERR(priv->rstmgr), "cannot map rstmgr\n");
		n = of_parse_phandle(np, "altr,sys-mgr", 0);
		if (!n || of_property_read_u32(n, "cpu1-start-addr", &start)) {
			of_node_put(n);
			return dev_err_probe(dev, -EINVAL,
					     "socfpga-a10 needs altr,sys-mgr with cpu1-start-addr\n");
		}
		of_node_put(n);
		priv->cpu1start = devm_ioremap(dev, start, sizeof(u32));
		if (!priv->cpu1start)
			return -ENOMEM;
		return 0;
	}
	return dev_err_probe(dev, -EINVAL, "unknown start method \"%s\"\n", method);
}

static int a9_rproc_probe(struct platform_device *pdev)
{
	struct device *dev = &pdev->dev;
	struct device_node *np = dev->of_node;
	struct a9_rproc *priv;
	struct rproc *rproc;
	const char *fw = NULL;
	int ret;

	of_property_read_string(np, "firmware-name", &fw);
	rproc = devm_rproc_alloc(dev, np->name, &a9_rproc_ops, fw, sizeof(*priv));
	if (!rproc)
		return -ENOMEM;

	priv = rproc->priv;
	priv->dev = dev;
	priv->rproc = rproc;
	priv->cpu = 1;
	priv->host_cpu = 0;
	priv->tx_sgi = 15;
	priv->rx_sgi = 14;
	of_property_read_u32(np, "openamp-rs,cpu", &priv->cpu);
	of_property_read_u32(np, "openamp-rs,host-cpu", &priv->host_cpu);
	of_property_read_u32(np, "openamp-rs,tx-sgi", &priv->tx_sgi);
	of_property_read_u32(np, "openamp-rs,rx-sgi", &priv->rx_sgi);
	if (priv->tx_sgi < 8 || priv->tx_sgi > 15 || priv->rx_sgi < 8 ||
	    priv->rx_sgi > 15 || priv->tx_sgi == priv->rx_sgi)
		return dev_err_probe(dev, -EINVAL, "SGIs must be distinct, in 8..15\n");
	if (priv->cpu > 7 || priv->cpu == priv->host_cpu)
		return dev_err_probe(dev, -EINVAL, "bad openamp-rs,cpu\n");
	if (of_property_read_u32(np, "openamp-rs,park-addr", &priv->park_addr))
		return dev_err_probe(dev, -EINVAL, "missing openamp-rs,park-addr\n");

	ret = a9_rproc_init_start_method(priv);
	if (ret)
		return ret;

	INIT_WORK(&priv->vq_work, a9_rproc_vq_work);
	ret = a9_rproc_init_gic(priv);
	if (ret)
		return ret;

	rproc->auto_boot = of_property_read_bool(np, "openamp-rs,auto-boot");
	platform_set_drvdata(pdev, rproc);
	ret = devm_rproc_add(dev, rproc);
	if (ret)
		return dev_err_probe(dev, ret, "rproc_add failed\n");
	dev_info(dev, "CPU%u as remote processor, SGI %u to it, SGI %u from it\n",
		 priv->cpu, priv->tx_sgi, priv->rx_sgi);
	return 0;
}

static void a9_rproc_remove(struct platform_device *pdev)
{
	struct rproc *rproc = platform_get_drvdata(pdev);
	struct a9_rproc *priv = rproc->priv;

	cancel_work_sync(&priv->vq_work);
}

static const struct of_device_id a9_rproc_of_match[] = {
	{ .compatible = "openamp-rs,cortex-a9-rproc" },
	{ }
};
MODULE_DEVICE_TABLE(of, a9_rproc_of_match);

static struct platform_driver a9_rproc_driver = {
	.probe = a9_rproc_probe,
	.remove = a9_rproc_remove,
	.driver = {
		.name = "a9-rproc",
		.of_match_table = a9_rproc_of_match,
	},
};
module_platform_driver(a9_rproc_driver);

MODULE_DESCRIPTION("Remoteproc driver for a spare Cortex-A9 core");
MODULE_LICENSE("GPL");
