#include <stdlib.h>

extern void init_container();

void hook(void)
{
    char *val;

    val = getenv("_RUNWASI_PHASE");
    if (val == NULL || *val != '1') {
        return;
    }

    init_container();
}