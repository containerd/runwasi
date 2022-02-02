#define _GNU_SOURCE
#include <fcntl.h>
#include <sched.h>

int setns_p(char *ns_path, int ns_type)
{
    int fd;


    int err;

    if (*ns_path == '\0')
    {
        err = unshare(ns_type);
    }
    else {
        fd = open(ns_path, O_RDONLY);
        if (fd < 0)
        {
            return fd;
        }
        err = setns(fd, ns_type);
    }

    if (err < 0)
    {
        return err;
    }
    return 0;
}