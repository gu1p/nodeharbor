pub fn guest_files() -> serde_json::Value {
    serde_json::json!([
        {"path":"/usr/local/lib/nodeharbor/configure_worker.py","owner":"root:root","permissions":"0700","content":include_str!("../../../guest/configure_worker.py")},
        {"path":"/usr/local/lib/nodeharbor/storage_pool.py","owner":"root:root","permissions":"0700","content":include_str!("../../../guest/storage_pool.py")},
        {"path":"/usr/local/lib/nodeharbor/watchdog.py","owner":"root:root","permissions":"0700","content":include_str!("../../../guest/watchdog.py")},
        {"path":"/etc/systemd/system/nodeharbor-watchdog.service","owner":"root:root","permissions":"0644","content":"[Unit]\nDescription=Check the NodeHarbor owner lease\n[Service]\nType=oneshot\nExecStart=/usr/bin/python3 /usr/local/lib/nodeharbor/watchdog.py\n"},
        {"path":"/etc/systemd/system/nodeharbor-watchdog.timer","owner":"root:root","permissions":"0644","content":"[Unit]\nDescription=Check the NodeHarbor owner lease regularly\n[Timer]\nOnActiveSec=15s\nOnUnitActiveSec=15s\nAccuracySec=1s\n[Install]\nWantedBy=timers.target\n"}
    ])
}
