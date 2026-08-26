/* C shim: libopenconnect's progress callback is variadic, which Rust cannot
 * define on stable. This function formats the message and forwards it to the
 * Rust callback stored in the first field of the privdata context. */
#include <stdarg.h>
#include <stdio.h>

typedef struct {
	void (*progress_fn)(void *privdata, int level, const char *msg);
} inode_oc_callbacks;

void inode_oc_progress_shim(void *privdata, int level, const char *fmt, ...)
{
	char buf[2048];
	va_list ap;

	if (!privdata)
		return;

	va_start(ap, fmt);
	vsnprintf(buf, sizeof(buf), fmt, ap);
	va_end(ap);

	((inode_oc_callbacks *)privdata)->progress_fn(privdata, level, buf);
}
