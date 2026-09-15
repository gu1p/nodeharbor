//! Reproducible native proof: no enrollment, credentials, production worker, or
//! network-policy changes. All images belong to fresh private test directories.
#![cfg(unix)]
use crate::{guest_files, storage::Location, storage_layout::Layout, LimaRunner, Runner, Vm};
use anyhow::{Context, Result};
use serde_json::json;
use std::{path::PathBuf, sync::Arc};

async fn guest(runner: &LimaRunner, script: &str) -> Result<()> {
    let args = [
        "shell",
        "--workdir=/",
        "worker",
        "sudo",
        "python3",
        "-c",
        script,
    ]
    .map(str::to_owned);
    let output = runner.run(&args, None, 120).await?;
    anyhow::ensure!(
        output.success,
        "Native guest proof failed: {}",
        output.stderr
    );
    Ok(())
}

#[tokio::test]
#[ignore = "Creates a disposable native Lima VM; requires NODEHARBOR_TEST_LIMA and distinct STORAGE_PRIMARY/SECONDARY test volumes"]
async fn native_selected_vm_moves_grows_and_preserves_data_without_an_enrolled_fleet() -> Result<()>
{
    let binary = PathBuf::from(std::env::var("NODEHARBOR_TEST_LIMA")?);
    let primary = tempfile::Builder::new()
        .prefix("nhv-")
        .tempdir_in(std::env::var("NODEHARBOR_TEST_STORAGE_PRIMARY")?)?;
    let settings = tempfile::Builder::new()
        .prefix("nhv-")
        .tempdir_in(std::env::var("NODEHARBOR_TEST_STORAGE_SECONDARY")?)?;
    let owner = uuid::Uuid::new_v4().to_string();
    let old_home = settings.path().join("lima");
    let runner = Arc::new(LimaRunner::new(binary.clone(), old_home.clone()));
    let old =
        Vm::managed(&owner, settings.path(), runner.clone())?.with_runtime_home(old_home.clone());
    let disk = Location {
        id: "nhoriginal".into(),
        directory: primary.path().to_string_lossy().into(),
        volume_id: crate::storage::volume_identity(primary.path())?,
        allocation_gib: 30,
    };
    anyhow::ensure!(
        disk.volume_id != crate::storage::volume_identity(settings.path())?,
        "Supply distinct mounted test filesystems"
    );
    let total = Location {
        allocation_gib: 100,
        ..disk.clone()
    };
    let target = Layout::resolve(&owner, &[total], None)?;
    target.validate_path()?;
    let moved_runner = Arc::new(LimaRunner::new(binary, target.home()));
    let moved = Vm::managed(&owner, settings.path(), moved_runner.clone())?
        .with_runtime_home(target.home());
    let new_disks = target.data_locations(&[Location {
        allocation_gib: 100,
        ..disk.clone()
    }])?;
    let result: Result<()> = async {
        let source_layout = Layout {runtime_directory:old_home.to_string_lossy().into(), volume_id:crate::storage::volume_identity(settings.path())?, ..target.clone()};
        crate::runtime_storage::prepare_home(&source_layout, &owner)?;
        let pinned: serde_json::Value = serde_json::from_str(include_str!("../../../runtime/lima.json"))?;
        let image = &pinned["images"][std::env::consts::ARCH];
        let image = crate::runtime_image::download(&source_layout, &owner, image["location"].as_str().context("Missing pinned image")?, image["digest"].as_str().context("Missing image digest")?, &|| Ok(())).await?;
        old.storage_adapter(settings.path())?.create(&disk).await?;
        old.clone().with_boot_image(image).create_with_storage(&nodeharbor_core::Resources {cpus:2,memory_mib:2048,disk_gib:46}, settings.path(), guest_files(), std::slice::from_ref(&disk), &owner, 1).await?;
        old.verify_owner().await?;
        println!("Native fixture: original VM prepared");
        guest(&runner, "import os,pathlib; p=pathlib.Path('/var/lib/nodeharbor/storage/relocation-proof'); f=p.open('wb'); f.write(b'NodeHarbor selected VM proof'*4096); f.flush(); os.fsync(f.fileno()); p.chmod(0o640)").await?;
        let before = old.storage_command("check", None).await?;
        old.stop().await?;
        crate::runtime_storage::verify_boot_image(&source_layout, &owner).await?;
        crate::runtime_image::cleanup(&source_layout, &owner)?;
        crate::runtime_storage::copy_runtime(&old_home, &target, &owner, std::slice::from_ref(&disk), &|| Ok(()))?;
        crate::runtime_storage::verify_boot_image(&target, &owner).await?;
        println!("Native fixture: system image copied onto selected volume");
        moved.storage_adapter(settings.path())?.grow(&disk, &new_disks[0]).await?;
        moved.set_storage_configuration(&new_disks, std::slice::from_ref(&disk), &owner, 2, true).await?;
        moved.start().await?;
        moved.prepare_storage_guest().await?;
        moved.storage_command("apply", Some(crate::lima::storage_request(&owner, &owner, &new_disks, std::slice::from_ref(&disk), 2))).await?;
        moved.storage_command("migrate", None).await?;
        let after = moved.storage_command("check", None).await?;
        let disk_info = moved.storage_adapter(settings.path())?.inspect(&new_disks[0]).await?;
        anyhow::ensure!(disk_info.size_bytes == 84_u64 << 30, "The physical data image does not match its allocation");
        println!("Native fixture: 16 GiB system + 84 GiB data images; guest filesystem {} bytes after overhead", after["capacityBytes"]);
        anyhow::ensure!(after["filesystemUuid"] == before["filesystemUuid"], "Moving/growing replaced the data filesystem");
        anyhow::ensure!(after["capacityBytes"].as_u64().is_some_and(|n| ((84_u64 << 30) * 85 / 100..=84_u64 << 30).contains(&n)), "The guest does not have the reviewed 84 GiB data disk");
        let proof = "import pathlib,stat; p=pathlib.Path('/var/lib/nodeharbor/storage/relocation-proof'); assert p.read_bytes()==b'NodeHarbor selected VM proof'*4096; assert stat.S_IMODE(p.stat().st_mode)==0o640";
        guest(&moved_runner, proof).await?;
        moved.stop().await?;
        crate::runtime_storage::verify_boot_image(&target, &owner).await?;
        crate::runtime_storage::verify_source(&old_home, &target, &owner)?;
        let removed = runner.run(&["delete".into(), "worker".into()], None, 90).await?;
        anyhow::ensure!(removed.success, "Cannot delete the verified original copy: {}", removed.stderr);
        crate::runtime_storage::remove_source_home(&old_home, &target, &owner)?;
        anyhow::ensure!(!old_home.exists(), "The original VM remains on the settings drive");
        anyhow::ensure!(moved.has_receipt()?, "Cleaning up the old runtime removed the active ownership receipt");
        moved.start().await?;
        moved.verify_owner().await?;
        guest(&moved_runner, proof).await?;
        moved.stop().await?;
        let extra = Location {id:"nhadded".into(), volume_id:crate::storage::volume_identity(settings.path())?, directory:settings.path().join("data").to_string_lossy().into(), allocation_gib:1};
        moved.storage_adapter(settings.path())?.create(&extra).await?;
        let mut added = new_disks.clone(); added.push(extra);
        moved.set_storage_configuration(&added, &new_disks, &owner, 3, true).await?;
        moved.start().await?;
        moved.storage_command("apply", Some(crate::lima::storage_request(&owner, &owner, &added, &new_disks, 3))).await?;
        let final_state = moved.storage_command("check", None).await?;
        anyhow::ensure!(final_state["capacityBytes"].as_u64().is_some_and(|n| n > after["capacityBytes"].as_u64().unwrap() + (1_u64 << 30) * 85 / 100), "The added selected location did not increase guest capacity");
        guest(&moved_runner, proof).await?;
        moved.stop().await?;
        for disk in &added { moved.storage_adapter(settings.path())?.inspect(disk).await?; }
        let hidden = target.home().with_extension("unavailable");
        std::fs::rename(target.home(), &hidden)?;
        let missing = crate::runtime_storage::validate_home(&target, &owner);
        anyhow::ensure!(!target.home().exists(), "Missing storage was recreated elsewhere");
        std::fs::rename(hidden, target.home())?;
        anyhow::ensure!(missing.is_err(), "Missing system storage was accepted");

        Ok(())
    }.await;
    // Keep failed fixtures stopped and owned so a failing proof cannot delete
    // its evidence or leave an untracked VM running.
    let mut cleanup_errors = Vec::new();
    for (home, runtime) in [(&old_home, &runner), (&target.home(), &moved_runner)] {
        let cleanup: Result<()> = async {
            if home.join("worker/lima.yaml").exists() {
                let instance = Vm::managed(&owner, settings.path(), runtime.clone())?
                    .with_runtime_home(home.clone());
                if !instance.info().await?.stopped {
                    instance.stop_now().await?;
                }
                anyhow::ensure!(
                    instance.info().await?.stopped,
                    "Cannot confirm stopped native fixture during cleanup"
                );
                if result.is_ok() {
                    let deleted = runtime
                        .run(&["delete".into(), "worker".into()], None, 90)
                        .await?;
                    anyhow::ensure!(
                        deleted.success && !instance.info().await?.installed,
                        "Cannot delete native test VM: {}",
                        deleted.stderr
                    );
                }
            }
            Ok(())
        }
        .await;
        if let Err(error) = cleanup {
            cleanup_errors.push(format!("{}: {error}", home.display()));
        }
    }
    if result.is_err() || !cleanup_errors.is_empty() {
        let first = primary.keep();
        let second = settings.keep();
        eprintln!(
            "Native evidence retained at {} and {}",
            first.display(),
            second.display()
        );
    }
    anyhow::ensure!(
        cleanup_errors.is_empty(),
        "Native result {result:?}; cleanup incomplete: {}",
        cleanup_errors.join("; ")
    );
    result?;
    println!(
        "NATIVE_SELECTED_VM_VERIFIED={}",
        json!({"totalGiB":100,"systemGiB":16,"dataGiB":84,"sourceRemoved":true,"restartDataPreserved":true,"additionalLocationVerified":true,"missingSystemRejected":true,"cleanupVerified":true})
    );
    Ok(())
}
