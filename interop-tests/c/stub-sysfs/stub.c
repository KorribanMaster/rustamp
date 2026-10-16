/* SPDX-License-Identifier: BSD-3-Clause */
#include <sysfs/libsysfs.h>
struct sysfs_attribute *sysfs_open_attribute(const char *p) { (void)p; return NULL; }
void sysfs_close_attribute(struct sysfs_attribute *a) { (void)a; }
int sysfs_read_attribute(struct sysfs_attribute *a) { (void)a; return -1; }
int sysfs_write_attribute(struct sysfs_attribute *a, const char *v, size_t l) { (void)a; (void)v; (void)l; return -1; }
struct sysfs_attribute *sysfs_get_device_attr(struct sysfs_device *d, const char *n) { (void)d; (void)n; return NULL; }
struct sysfs_attribute *sysfs_get_driver_attr(struct sysfs_driver *d, const char *n) { (void)d; (void)n; return NULL; }
struct sysfs_device *sysfs_open_device(const char *b, const char *i) { (void)b; (void)i; return NULL; }
void sysfs_close_device(struct sysfs_device *d) { (void)d; }
struct sysfs_driver *sysfs_open_driver(const char *b, const char *n) { (void)b; (void)n; return NULL; }
void sysfs_close_driver(struct sysfs_driver *d) { (void)d; }
struct sysfs_bus *sysfs_open_bus(const char *n) { (void)n; return NULL; }
void sysfs_close_bus(struct sysfs_bus *b) { (void)b; }
struct dlist *sysfs_open_directory_list(const char *p) { (void)p; return NULL; }
void sysfs_close_list(struct dlist *l) { (void)l; }
int sysfs_path_is_dir(const char *p) { (void)p; return -1; }
