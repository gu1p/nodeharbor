#![cfg(unix)]
use anyhow::{Context, Result};
use nodeharbor_agent::{
    guest_files, lima_storage::LimaStorage, storage::Location, LimaRunner, Runner, Vm,
};
use std::{path::PathBuf, sync::Arc};

#[tokio::test]
#[ignore = "Creates isolated file-backed Lima disks; requires pinned NODEHARBOR_TEST_LIMA and distinct NODEHARBOR_TEST_STORAGE_PRIMARY/SECONDARY volumes"]
async fn native_lima_disks_preserve_location_bytes_and_growth_across_runner_restarts() -> Result<()>
{
    let binary = PathBuf::from(std::env::var("NODEHARBOR_TEST_LIMA")?);
    let primary = PathBuf::from(std::env::var("NODEHARBOR_TEST_STORAGE_PRIMARY")?);
    let secondary = PathBuf::from(std::env::var("NODEHARBOR_TEST_STORAGE_SECONDARY")?);
    let first = tempfile::Builder::new()
        .prefix("nh724-disk-")
        .tempdir_in(primary)?;
    let second = tempfile::Builder::new()
        .prefix("nh724-disk-")
        .tempdir_in(secondary)?;
    let home = first.path().join("lima");
    let device = uuid::Uuid::new_v4().to_string();
    let runner = Arc::new(LimaRunner::new(binary.clone(), home.clone()));
    let storage = LimaStorage::new(runner, home.clone(), &device)?;
    let a = Location {
        id: "nh724a".into(),
        volume_id: nodeharbor_agent::storage::volume_identity(first.path())?,
        directory: first.path().to_string_lossy().into(),
        allocation_gib: 1,
    };
    let b = Location {
        id: "nh724b".into(),
        volume_id: nodeharbor_agent::storage::volume_identity(second.path())?,
        directory: second.path().to_string_lossy().into(),
        allocation_gib: 1,
    };
    let a_info = storage
        .create(&a)
        .await
        .context("Creating primary-volume disk")?;
    let b_info = storage
        .create(&b)
        .await
        .context("Creating second-volume disk")?;
    use std::os::unix::fs::MetadataExt;
    anyhow::ensure!(
        a_info.image.metadata()?.dev() != b_info.image.metadata()?.dev(),
        "Native test requires distinct mounted volumes"
    );
    let mut larger = b.clone();
    larger.allocation_gib = 2;
    storage.grow(&b, &larger).await?;
    let restarted = LimaStorage::new(
        Arc::new(LimaRunner::new(binary, home.clone())),
        home,
        &device,
    )?;
    anyhow::ensure!(
        restarted.inspect(&a).await?.size_bytes == 1 << 30,
        "Primary disk changed"
    );
    anyhow::ensure!(
        restarted.inspect(&larger).await?.size_bytes == 2 << 30,
        "Growth did not persist"
    );
    println!("Pinned native Lima registered distinct host-volume disk directories and retained growth across a fresh runner. No existing VM was used.");
    Ok(())
}

async fn guest(runner: &LimaRunner, command: &[&str]) -> Result<String> {
    let mut args = vec![
        "shell".into(),
        "--workdir=/".into(),
        "worker".into(),
        "sudo".into(),
    ];
    args.extend(command.iter().map(|arg| (*arg).to_owned()));
    let output = runner.run(&args, None, 120).await?;
    anyhow::ensure!(
        output.success,
        "Native guest command {command:?} failed: {} {}",
        output.stdout,
        output.stderr
    );
    Ok(output.stdout)
}

#[tokio::test]
#[ignore = "Boots an isolated plain Lima VM with two host-volume disks; NODEHARBOR_NATIVE_KEEP=1 retains only this test VM for follow-up qualification"]
async fn native_plain_vm_uses_a_persistent_ext4_pool_across_host_volumes() -> Result<()> {
    use serde_json::json;
    let binary = PathBuf::from(std::env::var("NODEHARBOR_TEST_LIMA")?);
    let primary = PathBuf::from(std::env::var("NODEHARBOR_TEST_STORAGE_PRIMARY")?);
    let secondary = PathBuf::from(std::env::var("NODEHARBOR_TEST_STORAGE_SECONDARY")?);
    let first = tempfile::Builder::new()
        .prefix("nh724-vm-")
        .tempdir_in(primary)?;
    let second = tempfile::Builder::new()
        .prefix("nh724-pool-")
        .tempdir_in(secondary)?;
    let home = first.path().join("lima");
    let device = uuid::Uuid::new_v4().to_string();
    let runner = Arc::new(LimaRunner::new(binary, home.clone()));
    let storage = LimaStorage::new(runner.clone(), home.clone(), &device)?;
    let vm = Vm::managed(&device, first.path(), runner.clone())?;
    let a = Location {
        id: "nh724a".into(),
        volume_id: nodeharbor_agent::storage::volume_identity(first.path())?,
        directory: first.path().to_string_lossy().into(),
        allocation_gib: 8,
    };
    let b = Location {
        id: "nh724b".into(),
        volume_id: nodeharbor_agent::storage::volume_identity(second.path())?,
        directory: second.path().to_string_lossy().into(),
        allocation_gib: 8,
    };
    let result: Result<()> = async {
        storage.create(&a).await?;
        storage.create(&b).await?;
        let mut files = guest_files();
        files.as_array_mut().context("Guest files must be an array")?.push(json!({
            "path":"/var/lib/kubelet/nh724-preserve","content":"native existing worker bytes","owner":"root:root","permissions":"0640"
        }));
        vm.create_with_storage(&nodeharbor_core::Resources { cpus:2,memory_mib:2048,disk_gib:16 }, first.path(), files, &[a.clone(), b.clone()], &device, 1).await?;
        vm.verify_owner().await?;
        let before = vm.storage_command("check", None).await?;
        anyhow::ensure!(before["capacityBytes"].as_u64().is_some_and(|size| size > 15_u64 << 30), "Pool capacity did not combine both disks");
        guest(&runner, &["python3", "-c", "import os,pathlib,stat; old=pathlib.Path('/var/lib/kubelet/nh724-preserve'); new=pathlib.Path('/var/lib/nodeharbor/storage/kubelet/nh724-preserve'); assert old.read_bytes()==new.read_bytes()==b'native existing worker bytes'; assert stat.S_IMODE(new.stat().st_mode)==0o640; assert new.stat().st_uid==0; p=pathlib.Path('/var/lib/nodeharbor/storage/native-proof'); f=p.open('wb'); f.write(b'nodeharbor-storage-proof'*1024); f.flush(); os.fsync(f.fileno())"]).await?;
        vm.stop().await?;
        vm.start().await?;
        vm.verify_owner().await?;
        vm.storage_command("activate", None).await?;
        let after = vm.storage_command("check", None).await?;
        anyhow::ensure!(before["filesystemUuid"] == after["filesystemUuid"], "Filesystem identity changed after VM restart");
        guest(&runner, &["python3", "-c", "from pathlib import Path; assert Path('/var/lib/nodeharbor/storage/native-proof').read_bytes()==b'nodeharbor-storage-proof'*1024"]).await?;
        println!("Plain Lima VM: both host-volume disks pooled into ext4; capacity, owner identity, legacy bytes/mode and filesystem UUID survived VM restart.");
        Ok(())
    }.await;
    if std::env::var("NODEHARBOR_NATIVE_KEEP").as_deref() == Ok("1") {
        let first = first.keep();
        let second = second.keep();
        println!(
            "NATIVE_STORAGE_FIXTURE={}",
            json!({"home":home,"primary":first,"secondary":second,"deviceId":device,"disks":[a,b]})
        );
        return result;
    }
    let cleanup: Result<()> = async {
        if vm.has_receipt()? {
            if vm.info().await?.running {
                vm.stop_now().await?;
            }
            vm.remove().await?;
        }
        Ok(())
    }
    .await;
    if let Err(error) = cleanup {
        let retained = first.keep();
        let secondary = second.keep();
        anyhow::bail!(
            "Native result {result:?}; cleanup failed: {error}; ownership retained at {} and {}",
            retained.display(),
            secondary.display()
        );
    }
    result
}

#[tokio::test]
#[ignore = "Continues only a retained task-owned native fixture from NODEHARBOR_NATIVE_FIXTURE; grows, adds, moves and temporarily hides its own disk directory"]
async fn native_pool_add_growth_move_and_missing_backing_preserve_bytes() -> Result<()> {
    use serde_json::{json, Value};
    let fixture: Value =
        serde_json::from_slice(&std::fs::read(std::env::var("NODEHARBOR_NATIVE_FIXTURE")?)?)?;
    let home = PathBuf::from(fixture["home"].as_str().context("No fixture home")?);
    let primary = PathBuf::from(fixture["primary"].as_str().context("No fixture primary")?);
    let secondary = PathBuf::from(
        fixture["secondary"]
            .as_str()
            .context("No fixture secondary")?,
    );
    let device = fixture["deviceId"].as_str().context("No fixture owner")?;
    let runner = Arc::new(LimaRunner::new(
        PathBuf::from(std::env::var("NODEHARBOR_TEST_LIMA")?),
        home.clone(),
    ));
    let vm = Vm::managed(device, &primary, runner.clone())?;
    vm.verify_owner().await?;
    let storage = LimaStorage::new(runner.clone(), home.clone(), device)?;
    let mut disks: Vec<Location> = serde_json::from_value(fixture["disks"].clone())?;
    anyhow::ensure!(disks.len() == 2, "Expected the original two-disk fixture");
    let before = vm.storage_command("check", None).await?;
    vm.stop().await?;
    let old = disks[1].clone();
    disks[1].allocation_gib += 1;
    storage.grow(&old, &disks[1]).await?;
    let added = Location {
        id: "nh724c".into(),
        volume_id: nodeharbor_agent::storage::volume_identity(&secondary)?,
        directory: secondary.to_string_lossy().into(),
        allocation_gib: 1,
    };
    storage.create(&added).await?;
    disks.push(added);
    vm.set_storage_attachments(&disks).await?;
    vm.start().await?;
    let request = json!({"format":1,"deviceId":device,"poolId":before["poolId"],"generation":before["generation"].as_u64().context("No generation")? + 1,
        "disks":disks.iter().enumerate().map(|(index,disk)|json!({"id":disk.id,"device":format!("/dev/vd{}", char::from(b'b'+index as u8)),"allocationBytes":disk.allocation_gib << 30,"initialize":index == 2})).collect::<Vec<_>>()});
    vm.storage_command("apply", Some(request)).await?;
    let larger = vm.storage_command("check", None).await?;
    anyhow::ensure!(
        larger["capacityBytes"]
            .as_u64()
            .context("No larger capacity")?
            > before["capacityBytes"]
                .as_u64()
                .context("No original capacity")?
                + (1 << 30),
        "Added and grown disks did not increase worker pool capacity"
    );
    anyhow::ensure!(
        larger["filesystemUuid"] == before["filesystemUuid"],
        "Growth replaced the filesystem"
    );
    vm.stop().await?;
    let old = disks[0].clone();
    let destination = tempfile::Builder::new()
        .prefix("nh724-moved-")
        .tempdir_in(&secondary)?
        .keep();
    disks[0].directory = destination.to_string_lossy().into();
    disks[0].volume_id = nodeharbor_agent::storage::volume_identity(&destination)?;
    println!(
        "NATIVE_MOVED_STORAGE={}",
        json!({"disks":disks,"retainedOriginal":old,"destination":destination})
    );
    storage.stage_move(&old, &disks[0]).await?;
    storage.commit_move(&old, &disks[0]).await?;
    vm.start().await?;
    vm.storage_command("activate", None).await?;
    let moved = vm.storage_command("check", None).await?;
    anyhow::ensure!(
        moved["filesystemUuid"] == before["filesystemUuid"],
        "Moving replaced the filesystem"
    );
    guest(&runner, &["python3", "-c", "from pathlib import Path; assert Path('/var/lib/nodeharbor/storage/native-proof').read_bytes()==b'nodeharbor-storage-proof'*1024"]).await?;
    vm.stop().await?;
    let paths = nodeharbor_agent::lima_storage::disk_paths(&home, device, &disks[0])?;
    let unavailable = paths.directory.with_extension("unavailable-test");
    std::fs::rename(&paths.directory, &unavailable)?;
    let refused = storage.inspect(&disks[0]).await.is_err();
    let unchanged =
        std::fs::read_link(&paths.link)? == paths.directory && !paths.directory.exists();
    std::fs::rename(&unavailable, &paths.directory)?;
    anyhow::ensure!(
        refused && unchanged,
        "Missing backing was not rejected without redirection"
    );
    vm.start().await?;
    vm.storage_command("activate", None).await?;
    let restored = vm.storage_command("check", None).await?;
    anyhow::ensure!(
        restored["filesystemUuid"] == before["filesystemUuid"],
        "Restored volume lost its filesystem identity"
    );
    println!("Native pool add/grow increased usable capacity; cross-volume move, missing-backing rejection and restoration preserved filesystem identity and bytes. Original image retained.");
    Ok(())
}
