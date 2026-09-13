/* Freestanding Linux ARM64 init used only by the isolated boot acceptance test. */
static long linux_call(long number, long a, long b, long c, long d) {
    register long x8 __asm__("x8") = number;
    register long x0 __asm__("x0") = a;
    register long x1 __asm__("x1") = b;
    register long x2 __asm__("x2") = c;
    register long x3 __asm__("x3") = d;
    __asm__ volatile("svc 0" : "+r"(x0) : "r"(x8), "r"(x1), "r"(x2), "r"(x3) : "memory", "cc");
    return x0;
}
void _start(void);
void _start(void) {
    static const char message[] = "NODEHARBOR_ISOLATED_BOOT_OK\n";
    linux_call(64, 1, (long)message, sizeof(message) - 1, 0);
    linux_call(142, 0xfee1dead, 672274793, 0x4321fedc, 0);
    for (;;) linux_call(94, 1, 0, 0, 0);
}
