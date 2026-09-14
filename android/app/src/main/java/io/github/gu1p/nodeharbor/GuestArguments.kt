package io.github.gu1p.nodeharbor

/** Owned files: writable root, kernel, initrd, seed, read-only root. */
fun guestArguments(policy: PhonePolicy, files: List<Int>, console: Int, control: Int, network: Int,
                   storage: List<Pair<Int, Int>> = emptyList()): Array<String> {
    require(files.size == 5) { "The worker needs its owned disk, kernel, initrd and seed" }
    require(storage.size <= 16) { "The worker supports at most sixteen additional disks" }
    val all = files + listOf(console, control, network) + storage.flatMap { listOf(it.first, it.second) }
    require(all.all { it >= 3 } && all.distinct().size == all.size) { "The worker needs distinct private descriptors" }
    require(policy.cpus in 1..64 && policy.memoryMib in 2048..65536) { "Unsupported worker resources" }
    return (listOf("nodeharbor-qemu", "-machine", "virt,gic-version=3", "-cpu", "cortex-a57",
        "-accel", "tcg,thread=multi", "-smp", policy.cpus.toString(), "-m", policy.memoryMib.toString(),
        "-nodefaults", "-no-user-config", "-display", "none", "-monitor", "none", "-no-reboot") +
        files.flatMapIndexed { index, fd -> listOf("-add-fd", "fd=$fd,set=${if (index == 4) 0 else index}") } + listOf(
        "-drive", "file=/dev/fdset/0,format=raw,if=none,id=root,cache=writeback",
        "-device", "virtio-blk-pci,drive=root", "-kernel", "/dev/fdset/1", "-initrd", "/dev/fdset/2",
        "-append", "console=ttyAMA0 root=/dev/vda rw panic=-1 " +
            "systemd.mask=fwupd.service systemd.mask=fwupd-refresh.service " +
            "systemd.mask=snapd.service systemd.mask=snapd.socket systemd.mask=snapd.seeded.service",
        ) + storage.flatMapIndexed { index, pair -> listOf(
            "-add-fd", "fd=${pair.first},set=${index + 4}", "-add-fd", "fd=${pair.second},set=${index + 4}",
            "-drive", "file=/dev/fdset/${index + 4},format=raw,if=none,id=storage$index,cache=writeback",
            "-device", "virtio-blk-pci,drive=storage$index") } + listOf(
        "-drive", "file=/dev/fdset/3,format=raw,if=none,id=seed,readonly=on", "-device", "virtio-blk-pci,drive=seed",
        "-chardev", "socket,id=console,fd=$console", "-serial", "chardev:console",
        "-device", "virtio-serial-pci", "-chardev", "socket,id=control,fd=$control",
        "-device", "virtserialport,chardev=control,name=io.github.gu1p.nodeharbor.control",
        "-object", "rng-builtin,id=entropy", "-device", "virtio-rng-pci,rng=entropy",
        "-netdev", "stream,id=net,addr.type=fd,addr.str=$network", "-device", "virtio-net-pci,netdev=net,romfile="
    )).toTypedArray()
}
