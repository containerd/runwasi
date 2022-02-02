#define _GNU_SOURCE
#include <stdlib.h>
#include <stdio.h>
#include <sched.h>

extern int setns_p(char*, int);

int setup_sandbox(void)
{
    char *val;
    int err;

    val = getenv("_RUNWASI_NETNS_PATH");
    if  (val != NULL)
    {
        err = setns_p(val, CLONE_NEWNET);
        if (err != 0)
        {
            return err;
        }
    }

    return 0;
}

void hook() {
    char *val;
    val = getenv("_RUNWASI_SANDBOX");
    if ((val != NULL) && (*val == '1'))
    {
       int err = setup_sandbox();
       if (err != 0)
       {
           perror("setup_sandbox");
       }
    }

    return;
}

