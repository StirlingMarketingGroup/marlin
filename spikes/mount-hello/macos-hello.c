// Evidence for docs/decisions/0001: libfuse 3's low-level API on macFUSE.
// A read of slow.txt is answered three seconds later from another thread.
#define FUSE_USE_VERSION 34
#include <fuse_lowlevel.h>
#include <errno.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define HELLO_INO 2
#define SLOW_INO 3
static const char *HELLO = "Hello from Marlin\n";
static const char *SLOW = "slow\n";

// Set while a read of slow.txt is waiting for its answer.
static atomic_bool slow_pending;

// The run script requires this line, so a pass means a callback really ran during the slow read.
static void log_if_slow_pending(const char *callback) {
	if (atomic_load(&slow_pending))
		fprintf(stderr, "callback during slow read: %s\n", callback);
}

static int fill_stat(fuse_ino_t ino, struct stat *st) {
	memset(st, 0, sizeof(*st));
	st->st_ino = ino;
	st->st_uid = getuid();
	st->st_gid = getgid();
	if (ino == FUSE_ROOT_ID) {
		st->st_mode = S_IFDIR | 0555;
		st->st_nlink = 2;
	} else if (ino == HELLO_INO || ino == SLOW_INO) {
		st->st_mode = S_IFREG | 0444;
		st->st_nlink = 1;
		st->st_size = strlen(ino == HELLO_INO ? HELLO : SLOW);
	} else {
		return -1;
	}
	return 0;
}

static void ll_getattr(fuse_req_t req, fuse_ino_t ino, struct fuse_file_info *fi) {
	struct stat st;
	(void)fi;
	log_if_slow_pending("getattr");
	if (fill_stat(ino, &st) == -1)
		fuse_reply_err(req, ENOENT);
	else
		fuse_reply_attr(req, &st, 1.0);
}

static void ll_lookup(fuse_req_t req, fuse_ino_t parent, const char *name) {
	struct fuse_entry_param e;
	log_if_slow_pending("lookup");
	memset(&e, 0, sizeof(e));
	if (parent == FUSE_ROOT_ID && strcmp(name, "hello.txt") == 0)
		e.ino = HELLO_INO;
	else if (parent == FUSE_ROOT_ID && strcmp(name, "slow.txt") == 0)
		e.ino = SLOW_INO;
	else {
		fuse_reply_err(req, ENOENT);
		return;
	}
	e.attr_timeout = 1.0;
	e.entry_timeout = 1.0;
	fill_stat(e.ino, &e.attr);
	fuse_reply_entry(req, &e);
}

static void ll_readdir(fuse_req_t req, fuse_ino_t ino, size_t size, off_t off, struct fuse_file_info *fi) {
	static const char *names[] = {".", "..", "hello.txt", "slow.txt"};
	static const fuse_ino_t inos[] = {FUSE_ROOT_ID, FUSE_ROOT_ID, HELLO_INO, SLOW_INO};
	char buf[1024];
	size_t used = 0;
	(void)fi;
	log_if_slow_pending("readdir");
	if (ino != FUSE_ROOT_ID) {
		fuse_reply_err(req, ENOTDIR);
		return;
	}
	for (off_t i = off; i < 4; i++) {
		struct stat st;
		fill_stat(inos[i], &st);
		size_t need = fuse_add_direntry(req, buf + used, sizeof(buf) - used, names[i], &st, i + 1);
		if (need > sizeof(buf) - used || used + need > size)
			break;
		used += need;
	}
	fuse_reply_buf(req, buf, used);
}

static void ll_open(fuse_req_t req, fuse_ino_t ino, struct fuse_file_info *fi) {
	if (ino != HELLO_INO && ino != SLOW_INO)
		fuse_reply_err(req, EISDIR);
	else
		fuse_reply_open(req, fi);
}

static void reply_slice(fuse_req_t req, const char *data, size_t size, off_t off) {
	size_t len = strlen(data);
	if ((size_t)off >= len)
		fuse_reply_buf(req, NULL, 0);
	else
		fuse_reply_buf(req, data + off, len - off < size ? len - off : size);
}

struct slow_read {
	fuse_req_t req;
	size_t size;
	off_t off;
};

static void *answer_slow_read(void *arg) {
	struct slow_read *slow = arg;
	sleep(3);
	reply_slice(slow->req, SLOW, slow->size, slow->off);
	atomic_store(&slow_pending, false);
	free(slow);
	return NULL;
}

static void ll_read(fuse_req_t req, fuse_ino_t ino, size_t size, off_t off, struct fuse_file_info *fi) {
	(void)fi;
	if (ino != SLOW_INO) {
		log_if_slow_pending("read");
		reply_slice(req, HELLO, size, off);
		return;
	}
	// The run script waits for this line before it makes its fast calls.
	atomic_store(&slow_pending, true);
	fprintf(stderr, "slow read pending\n");
	// Return to the session loop at once. Another thread answers later.
	struct slow_read *slow = malloc(sizeof(*slow));
	pthread_t thread;
	slow->req = req;
	slow->size = size;
	slow->off = off;
	pthread_create(&thread, NULL, answer_slow_read, slow);
	pthread_detach(thread);
}

static const struct fuse_lowlevel_ops ops = {
	.lookup = ll_lookup,
	.getattr = ll_getattr,
	.readdir = ll_readdir,
	.open = ll_open,
	.read = ll_read,
};

int main(int argc, char *argv[]) {
	struct fuse_args args = FUSE_ARGS_INIT(argc, argv);
	struct fuse_cmdline_opts opts;
	struct fuse_session *se;
	int ret = -1;

	if (fuse_parse_cmdline(&args, &opts) != 0 || opts.mountpoint == NULL) {
		fprintf(stderr, "usage: %s [-o options] <mountpoint>\n", argv[0]);
		return 2;
	}
	se = fuse_session_new(&args, &ops, sizeof(ops), NULL);
	if (se == NULL)
		return 1;
	if (fuse_set_signal_handlers(se) == 0 && fuse_session_mount(se, opts.mountpoint) == 0) {
		printf("mounted %s\n", opts.mountpoint);
		fflush(stdout);
		// One loop thread on purpose: a slow read must not need a second one.
		ret = fuse_session_loop(se);
		fuse_session_unmount(se);
	}
	fuse_remove_signal_handlers(se);
	fuse_session_destroy(se);
	// The loop returns 0 on unmount, a signal number on a signal, and a negative errno on failure.
	return ret < 0;
}
