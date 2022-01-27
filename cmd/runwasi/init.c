#define _GNU_SOURCE
#include <stdlib.h>
#include <stdio.h>
#include <sched.h>

#ifndef CLONE_NEWNET
    #define CLONE_NEWNET 0x40000000
#endif

void init_container(void)
{
    char *val;

    // We run setns from here because go really can't do this without locking everything to one goroutine, which we can't do.
    val = getenv("_RUNWASI_NETNS_PATH");
    if (val != NULL) {
        int err;

        err = setns(4, CLONE_NEWNET);
        if (err < 0)
        {
            perror("setns");
        }
    }
}