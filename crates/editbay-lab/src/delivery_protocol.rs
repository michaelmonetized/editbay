use crate::Result;
use editbay_media::{Cancellation, native_job::NativeJob};
use rustix::fs::{Mode, OFlags, openat};
use serde_json::{Value, json};
use std::{fs::File, path::Path, time::Instant};
use uuid::Uuid;

/// Send stale, foreign and malformed operations to actual packaged workers.
/// `project` and `composition` supply a valid binding. Returns protocol rejection,
/// descriptor cleanup and process retirement evidence without published output.
pub fn run(project: &Path, composition: &str) -> Result<Value> {
    let project = editbay_core::load(project)?;
    let directory = tempfile::tempdir()?;
    let parent = File::open(directory.path())?;
    let mut reports = Vec::new();
    for mode in [
        "malformed",
        "unknown-field",
        "missing-file",
        "named-file",
        "stale",
        "foreign",
        "foreign-version",
        "extra-file",
        "duplicate-begin",
        "empty-range",
        "reversed-range",
        "foreign-range",
        "overflow-range",
    ] {
        let file = if mode == "named-file" {
            File::create_new(directory.path().join("owned-original.mov"))?
        } else {
            File::from(openat(
                &parent,
                ".",
                OFlags::TMPFILE | OFlags::RDWR | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )?)
        };
        let cancel = Cancellation::new()?;
        let mut child = NativeJob::<Value>::start(&std::env::current_exe()?, "--delivery-worker")?;
        let pid = child.process_id();
        let owner =
            json!({"job":Uuid::new_v4(),"version":editbay_core::DocumentVersion::of(&project)});
        let begin = json!({"owner":owner,"serial":0,"operation":{"kind":"begin","project":project,"request":{"composition":composition,"sample_rate":48000,"range":{"start":2,"end":7}}}});
        let (request, descriptor) = match mode {
            "empty-range" | "reversed-range" | "foreign-range" | "overflow-range" => {
                let mut request = begin.clone();
                request["operation"]["request"]["range"] = match mode {
                    "empty-range" => json!({"start":2,"end":2}),
                    "reversed-range" => json!({"start":7,"end":2}),
                    "foreign-range" => json!({"start":0,"end":u64::MAX}),
                    _ => json!({"start":u64::MAX-1,"end":u64::MAX}),
                };
                (request, Some(&file))
            }
            "malformed" => (json!("invalid request"), None),
            "unknown-field" => {
                let mut request = begin.clone();
                request["foreign"] = json!(true);
                (request, Some(&file))
            }
            "missing-file" => (begin, None),
            "named-file" => (begin, Some(&file)),
            _ => {
                let reply = child.request(&begin, Some(&file), &cancel)?;
                if reply["outcome"]["progress"]["phase"] != "rendering" {
                    return Err(format!("Protocol trial could not bind: {reply}").into());
                }
                let mut request = json!({"owner":owner,"serial":1,"operation":{"kind":"step"}});
                match mode {
                    "stale" => request["serial"] = json!(0),
                    "foreign" => request["owner"]["job"] = json!(Uuid::new_v4()),
                    "foreign-version" => {
                        request["owner"]["version"]["revision"] = json!(project.revision + 1)
                    }
                    "duplicate-begin" => request["operation"] = begin["operation"].clone(),
                    _ => {}
                }
                (request, (mode == "extra-file").then_some(&file))
            }
        };
        let began = Instant::now();
        let reply = child.request(&request, descriptor, &cancel);
        let rejected = reply.as_ref().is_err()
            || reply
                .as_ref()
                .is_ok_and(|reply| reply["outcome"]["kind"] == "failed");
        let error = match reply {
            Ok(reply) => reply,
            Err(error) => json!(error.to_string()),
        };
        drop(child);
        let retired = !Path::new(&format!("/proc/{pid}")).exists();
        if !rejected || !retired || began.elapsed().as_secs_f64() > 2. {
            return Err(format!("Protocol {mode} failed: {error}").into());
        }
        reports.push(json!({"mode":mode,"rejected":rejected,"retired":retired,"retirement_ms":began.elapsed().as_secs_f64()*1000.,"error":error}));
    }
    if std::fs::read_dir(directory.path())?.count() != 1
        || std::fs::metadata(directory.path().join("owned-original.mov"))?.len() != 0
    {
        return Err("Rejected delivery changed files".into());
    }
    Ok(
        json!({"kind":"shared_delivery_worker_protocol","qualified":true,"reports":reports,"application_sha256":crate::hash(&std::env::current_exe()?)?}),
    )
}
