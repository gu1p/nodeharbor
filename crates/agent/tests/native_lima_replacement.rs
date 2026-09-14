//! Standalone native storage proof. This creates its own unenrolled VM and never
//! adopts an ordinary worker. Fleet admission/Kubernetes qualification still need
//! the separate enrolled lifecycle fixture on every required host architecture.
#![cfg(any(target_os = "macos", target_os = "linux"))]
use anyhow::{Context, Result};
use nodeharbor_agent::{
    guest_files,
    storage::Location,
    storage_lifecycle::{checksum, verified_backup, Backup},
    LimaRunner, Runner, Vm,
};
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};

#[tokio::test]
#[ignore = "Creates an isolated 30 GiB Lima pool on two explicitly supplied volumes, then backs up and replaces it with 15 GiB; requires NODEHARBOR_TEST_LIMA and NODEHARBOR_TEST_STORAGE_PRIMARY/SECONDARY"]
async fn native_replacement_preserves_data_with_a_fresh_pool_and_a_slow_host_verification(
) -> Result<()> {
    let first = tempfile::Builder::new()
        .prefix("nhsr-")
        .tempdir_in(std::env::var("NODEHARBOR_TEST_STORAGE_PRIMARY")?)?;
    let second = tempfile::Builder::new()
        .prefix("nhsr-")
        .tempdir_in(std::env::var("NODEHARBOR_TEST_STORAGE_SECONDARY")?)?;
    let device = uuid::Uuid::new_v4().to_string();
    let pool = uuid::Uuid::new_v4().to_string();
    let runner = Arc::new(LimaRunner::new(
        PathBuf::from(std::env::var("NODEHARBOR_TEST_LIMA")?),
        first.path().join("lima"),
    ));
    let vm = Vm::managed(&device, first.path(), runner.clone())?;
    let storage = vm.storage_adapter(first.path())?;
    let a = Location {
        id: "nhreviewa".into(),
        directory: first.path().to_string_lossy().into(),
        volume_id: nodeharbor_agent::storage::volume_identity(first.path())?,
        allocation_gib: 15,
    };
    let b = Location {
        id: "nhreviewb".into(),
        directory: second.path().to_string_lossy().into(),
        volume_id: nodeharbor_agent::storage::volume_identity(second.path())?,
        allocation_gib: 15,
    };
    anyhow::ensure!(
        a.volume_id != b.volume_id,
        "Supply two distinct test volumes"
    );
    let target = Location {
        id: "nhreviewnew".into(),
        ..b.clone()
    };
    let guest = |script: &str| {
        let args: Vec<_> = [
            "shell",
            "--workdir=/",
            "worker",
            "sudo",
            "python3",
            "-c",
            script,
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let runner = runner.clone();
        async move {
            let output = runner.run(&args, None, 120).await?;
            anyhow::ensure!(
                output.success,
                "Native guest check failed: {}",
                output.stderr
            );
            Ok::<_, anyhow::Error>(())
        }
    };
    let outcome=tokio::time::timeout(Duration::from_secs(3600),async {
        storage.create(&a).await?;storage.create(&b).await?;
        vm.create_with_storage(&nodeharbor_core::Resources{cpus:2,memory_mib:2048,disk_gib:30},first.path(),guest_files(),&[a.clone(),b.clone()],&pool,1).await?;
        vm.prepare_storage_guest().await?;
        let before=vm.storage_command("check",None).await?;
        anyhow::ensure!(before["poolId"]==pool,"First boot did not use the durable pool identity");
        guest("import os,pathlib,subprocess\np=pathlib.Path('/var/lib/nodeharbor/storage/native-review');p.mkdir();f=p/'payload';f.write_bytes(b'preserved bytes');f.chmod(0o640);os.chown(f,12345,23456);os.setxattr(f,'user.nodeharbor',b'metadata');os.link(f,p/'hardlink');(p/'symlink').symlink_to('payload')\nwith (p/'sparse').open('wb') as h:h.seek(1024**3);h.write(b'end')\nsubprocess.run(['systemctl','daemon-reload'],check=True);subprocess.run(['systemctl','enable','--now','nodeharbor-watchdog.timer'],check=True)").await?;
        let archive=tempfile::NamedTempFile::new_in(first.path())?;
        vm.backup_stream("backup",None,Some(archive.reopen()?),1<<30).await?;
        let file=archive.reopen()?;
        // A controlled host delay exceeds the real guest watchdog's 120s lease.
        // The timer is active; this proves lease supervision independently of I/O speed.
        let (hash,bytes)=vm.with_owner_lease(async move {
            tokio::time::sleep(Duration::from_secs(150)).await;
            tokio::task::spawn_blocking(move||checksum(file)).await?
        }).await?;
        anyhow::ensure!(vm.info().await?.reachable,"Guest watchdog expired during host verification");
        vm.backup_stream("verify",Some(archive.reopen()?),None,0).await?;
        let backup=Backup{path:archive.path().to_string_lossy().into(),volume_id:a.volume_id.clone(),bytes,sha256:Some(hash),verified:true};
        vm.storage_command("retire",Some(json!({"poolId":pool}))).await?;vm.stop().await?;
        vm.set_storage_attachments(&[]).await?;storage.remove_owned(&a).await?;storage.remove_owned(&b).await?;
        storage.create(&target).await?;let replacement=uuid::Uuid::new_v4().to_string();
        for _ in 0..2 {vm.set_storage_configuration(std::slice::from_ref(&target),&[],&replacement,2,false).await?;}
        vm.start().await?;vm.prepare_storage_guest().await?;
        let file=verified_backup(&backup)?;
        vm.backup_stream("restore",Some(file),None,0).await?;
        vm.backup_stream("verify",Some(verified_backup(&backup)?),None,0).await?;
        vm.storage_command("restored",Some(json!({"poolId":replacement}))).await?;
        let after=vm.storage_command("check",None).await?;
        anyhow::ensure!(after["poolId"]==replacement && after["generation"]==2 && after["filesystemUuid"]!=before["filesystemUuid"],"Replacement identities did not match");
        anyhow::ensure!(after["capacityBytes"].as_u64().is_some_and(|bytes|((15_u64<<30)*85/100..=15_u64<<30).contains(&bytes)),"Replacement did not expose its reduced guest capacity");
        vm.stop().await?;vm.start().await?;vm.renew_lease().await?;
        guest("import os,pathlib,stat\np=pathlib.Path('/var/lib/nodeharbor/storage/native-review');f=p/'payload';assert f.read_bytes()==b'preserved bytes';assert f.stat().st_uid==12345 and f.stat().st_gid==23456;assert stat.S_IMODE(f.stat().st_mode)==0o640;assert f.stat().st_ino==(p/'hardlink').stat().st_ino;assert os.readlink(p/'symlink')=='payload';assert os.getxattr(f,'user.nodeharbor')==b'metadata';assert (p/'sparse').stat().st_blocks*512<1024**2\nwith (p/'sparse').open('rb') as h:h.seek(1024**3);assert h.read()==b'end'").await?;
        Ok::<_,anyhow::Error>(())
    }).await.context("Native replacement timed out").and_then(|result|result);
    let cleanup = async {
        if vm.has_receipt()? {
            if vm.info().await?.running {
                vm.stop_now().await?;
            }
            vm.remove().await?;
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if let Err(error) = cleanup {
        let first = first.keep();
        let second = second.keep();
        anyhow::bail!("Native result {outcome:?}; cleanup failed: {error}; owned fixtures retained at {} and {}",first.display(),second.display());
    }
    outcome
}
