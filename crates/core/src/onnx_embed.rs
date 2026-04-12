use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, ChildStdout};
use std::sync::{Arc, Mutex};

struct SharedProcess {
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    started: bool,
}

pub struct OnnxModel {
    script_path: PathBuf,
    shared: Arc<Mutex<SharedProcess>>,
}

impl OnnxModel {
    pub fn load() -> anyhow::Result<Self> {
        let script_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("onnx_embed_python.py");

        if !script_path.exists() {
            anyhow::bail!(
                "Python embedding script not found at {}. Run Python benchmark first to download.",
                script_path.display()
            );
        }

        let shared = Arc::new(Mutex::new(SharedProcess {
            stdin: None,
            stdout: None,
            started: false,
        }));

        Ok(Self {
            script_path,
            shared,
        })
    }

    pub fn load_from_dir(_model_dir: &Path) -> anyhow::Result<Self> {
        Self::load()
    }

    fn ensure_started(&self) -> anyhow::Result<()> {
        let mut shared = self.shared.lock().unwrap();

        if shared.started {
            return Ok(());
        }

        let mut cmd = std::process::Command::new("python3");
        cmd.arg("-u")
            .arg(&self.script_path)
            .arg("--persistent")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().unwrap();
        let stdin = child.stdin.take().unwrap();

        shared.stdin = Some(stdin);
        shared.stdout = Some(stdout);
        shared.started = true;

        let stdin_ptr: *mut ChildStdin = shared.stdin.as_mut().unwrap();
        let stdout_ptr: *mut ChildStdout = shared.stdout.as_mut().unwrap();
        drop(shared);

        unsafe {
            writeln!(&mut *stdin_ptr, "PERSISTENT")?;
            stdin_ptr.as_mut().unwrap().flush()?;

            let mut reader = BufReader::new(&mut *stdout_ptr);
            let mut line = String::new();
            reader.read_line(&mut line)?;
            if !line.contains("OK") {
                anyhow::bail!("Failed to start persistent mode: {}", line);
            }
        }

        Ok(())
    }

    pub fn encode(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let embeddings = self.encode_batch(&[text], false)?;
        Ok(embeddings.into_iter().next().unwrap())
    }

    pub fn encode_batch(&self, texts: &[&str], _normalize: bool) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        self.ensure_started()?;

        let input = serde_json::to_string(texts)?;

        let stdin_ptr: *mut ChildStdin;
        let stdout_ptr: *mut ChildStdout;
        {
            let mut shared = self.shared.lock().unwrap();
            stdin_ptr = shared.stdin.as_mut().unwrap();
            stdout_ptr = shared.stdout.as_mut().unwrap();
        }

        unsafe {
            writeln!(&mut *stdin_ptr, "{}", input)?;
            stdin_ptr.as_mut().unwrap().flush()?;

            let mut results = Vec::new();
            let mut reader = BufReader::new(&mut *stdout_ptr);
            for _ in 0..texts.len() {
                let mut line = String::new();
                reader.read_line(&mut line)?;
                let line = line.trim();
                if line.starts_with("ERROR:") {
                    anyhow::bail!("Python embedding error: {}", line);
                }
                let embedding: Vec<f32> = serde_json::from_str(line)?;
                results.push(embedding);
            }

            let mut done = String::new();
            reader.read_line(&mut done)?;

            Ok(results)
        }
    }

    pub fn dimension(&self) -> usize {
        384
    }
}

impl Clone for OnnxModel {
    fn clone(&self) -> Self {
        Self {
            script_path: self.script_path.clone(),
            shared: Arc::clone(&self.shared),
        }
    }
}

impl Drop for OnnxModel {
    fn drop(&mut self) {
        if let Ok(mut shared) = self.shared.lock() {
            if shared.started {
                if let Some(ref mut stdin) = shared.stdin {
                    let _ = stdin.write_all(b"QUIT\n");
                }
            }
        }
    }
}
