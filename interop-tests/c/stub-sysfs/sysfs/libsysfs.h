/* SPDX-License-Identifier: BSD-3-Clause */
/*
 * Minimal stand-in for libsysfs so the Linux flavour of libmetal builds on
 * machines without libsysfs-dev. The interop tests never call metal_init()
 * or the device/bus API, so every function simply fails.
 */
#ifndef STUB_LIBSYSFS_H
#define STUB_LIBSYSFS_H
#include <stddef.h>
#include <string.h>
#include <stdlib.h>

#define SYSFS_PATH_MAX 256
#define SYSFS_UNKNOWN "unknown"

struct dlist { int unused; };
#define dlist_for_each_data(list, data, type) \
	for ((data) = NULL; (data) != NULL; )

struct sysfs_attribute { char *value; size_t len; };
struct sysfs_device { char path[SYSFS_PATH_MAX]; char driver_name[SYSFS_PATH_MAX]; };
struct sysfs_driver { char path[SYSFS_PATH_MAX]; };
struct sysfs_bus { char path[SYSFS_PATH_MAX]; };

struct sysfs_attribute *sysfs_open_attribute(const char *path);
void sysfs_close_attribute(struct sysfs_attribute *a);
int sysfs_read_attribute(struct sysfs_attribute *a);
int sysfs_write_attribute(struct sysfs_attribute *a, const char *v, size_t len);
struct sysfs_attribute *sysfs_get_device_attr(struct sysfs_device *d, const char *n);
struct sysfs_attribute *sysfs_get_driver_attr(struct sysfs_driver *d, const char *n);
struct sysfs_device *sysfs_open_device(const char *bus, const char *id);
void sysfs_close_device(struct sysfs_device *d);
struct sysfs_driver *sysfs_open_driver(const char *bus, const char *name);
void sysfs_close_driver(struct sysfs_driver *d);
struct sysfs_bus *sysfs_open_bus(const char *name);
void sysfs_close_bus(struct sysfs_bus *b);
struct dlist *sysfs_open_directory_list(const char *path);
void sysfs_close_list(struct dlist *l);
int sysfs_path_is_dir(const char *path);
#endif
