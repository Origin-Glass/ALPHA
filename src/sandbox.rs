use std::{
    collections::BTreeMap, io, os::unix::fs::PermissionsExt, path::Path, process::Stdio,
    time::Duration,
};

use crate::judge::{Checker, JudgeTestCase, LeasedJob, Verdict};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::mpsc,
};

const STDOUT_LIMIT: usize = 1_048_576;
const STDERR_LIMIT: usize = 131_072;
const COMPILE_OUTPUT_LIMIT: usize = 16_384;

#[derive(Debug)]
pub enum SandboxError {
    InvalidConfiguration,
    Io(io::Error),
    DockerUnavailable,
}

impl std::fmt::Display for SandboxError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfiguration => {
                formatter.write_str("판정 샌드박스 설정이 올바르지 않습니다")
            }
            Self::Io(error) => write!(formatter, "판정 작업 공간 오류: {error}"),
            Self::DockerUnavailable => {
                formatter.write_str("Docker 판정 런타임을 사용할 수 없습니다")
            }
        }
    }
}

impl std::error::Error for SandboxError {}

impl From<io::Error> for SandboxError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug)]
pub struct JudgeOutcome {
    pub verdict: Verdict,
    pub score: i16,
    pub compile_output: Option<String>,
}

pub struct DockerSandbox {
    docker_binary: String,
    image: String,
}

enum ContainerResult {
    Exited {
        success: bool,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        oom_killed: bool,
    },
    Timeout,
    OutputLimit,
    SystemError,
}

struct ExecutionLimits {
    memory_mb: i32,
    timeout: Duration,
    stdout_bytes: usize,
    stderr_bytes: usize,
}

async fn read_limited<R: AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
    overflow: mpsc::UnboundedSender<()>,
) -> (Vec<u8>, bool) {
    let mut result = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0_u8; 8192];
    let mut exceeded = false;
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let remaining = limit.saturating_sub(result.len());
                result.extend_from_slice(&buffer[..read.min(remaining)]);
                if read > remaining && !exceeded {
                    exceeded = true;
                    let _ = overflow.send(());
                }
            }
        }
    }
    (result, exceeded)
}

impl DockerSandbox {
    pub fn new(docker_binary: String, image: String) -> Result<Self, SandboxError> {
        if docker_binary.is_empty()
            || image.is_empty()
            || image.bytes().any(|byte| byte.is_ascii_whitespace())
        {
            return Err(SandboxError::InvalidConfiguration);
        }
        Ok(Self {
            docker_binary,
            image,
        })
    }

    pub async fn verify(&self) -> Result<(), SandboxError> {
        let status = Command::new(&self.docker_binary)
            .args(["image", "inspect", &self.image])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map_err(|_| SandboxError::DockerUnavailable)?;
        if status.success() {
            Ok(())
        } else {
            Err(SandboxError::DockerUnavailable)
        }
    }

    pub fn image_reference(&self) -> &str {
        &self.image
    }

    fn docker_args(
        &self,
        name: &str,
        workspace: &Path,
        memory_mb: i32,
        command: &[String],
    ) -> Result<Vec<String>, SandboxError> {
        let workspace = workspace
            .to_str()
            .ok_or(SandboxError::InvalidConfiguration)?;
        let memory = format!("{}m", memory_mb.clamp(64, 2048));
        let mut args = vec![
            "run".to_owned(),
            "--name".to_owned(),
            name.to_owned(),
            "--network".to_owned(),
            "none".to_owned(),
            "--read-only".to_owned(),
            "--tmpfs".to_owned(),
            "/tmp:rw,noexec,nosuid,nodev,size=64m".to_owned(),
            "--cap-drop".to_owned(),
            "ALL".to_owned(),
            "--security-opt".to_owned(),
            "no-new-privileges:true".to_owned(),
            "--pids-limit".to_owned(),
            "64".to_owned(),
            "--memory".to_owned(),
            memory.clone(),
            "--memory-swap".to_owned(),
            memory,
            "--cpus".to_owned(),
            "1".to_owned(),
            "--ulimit".to_owned(),
            "nofile=64:64".to_owned(),
            "--ulimit".to_owned(),
            "nproc=64:64".to_owned(),
            "--ulimit".to_owned(),
            "fsize=67108864:67108864".to_owned(),
            "--user".to_owned(),
            "65534:65534".to_owned(),
            "--workdir".to_owned(),
            "/workspace".to_owned(),
            "--volume".to_owned(),
            format!("{workspace}:/workspace:rw"),
            self.image.clone(),
        ];
        args.extend(command.iter().cloned());
        Ok(args)
    }

    async fn cleanup(&self, name: &str) {
        let _ = Command::new(&self.docker_binary)
            .args(["rm", "--force", name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }

    async fn oom_killed(&self, name: &str) -> bool {
        Command::new(&self.docker_binary)
            .args(["inspect", "--format", "{{.State.OOMKilled}}", name])
            .output()
            .await
            .ok()
            .is_some_and(|output| output.status.success() && output.stdout == b"true\n")
    }

    async fn execute_container(
        &self,
        name: &str,
        workspace: &Path,
        command: &[String],
        input: &[u8],
        limits: ExecutionLimits,
    ) -> Result<ContainerResult, SandboxError> {
        self.cleanup(name).await;
        let args = self.docker_args(name, workspace, limits.memory_mb, command)?;
        let mut child = Command::new(&self.docker_binary)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| SandboxError::DockerUnavailable)?;
        let mut stdin = child.stdin.take().ok_or(SandboxError::DockerUnavailable)?;
        let input = input.to_vec();
        let input_task = tokio::spawn(async move {
            let _ = stdin.write_all(&input).await;
            let _ = stdin.shutdown().await;
        });
        let stdout = child.stdout.take().ok_or(SandboxError::DockerUnavailable)?;
        let stderr = child.stderr.take().ok_or(SandboxError::DockerUnavailable)?;
        let (overflow_tx, mut overflow_rx) = mpsc::unbounded_channel();
        let stdout_task = tokio::spawn(read_limited(
            stdout,
            limits.stdout_bytes,
            overflow_tx.clone(),
        ));
        let stderr_task = tokio::spawn(read_limited(stderr, limits.stderr_bytes, overflow_tx));

        let result = tokio::select! {
            status = child.wait() => match status {
                Ok(status) => {
                    let (_, _) = tokio::join!(input_task, async {});
                    let stdout = stdout_task.await.unwrap_or_default();
                    let stderr = stderr_task.await.unwrap_or_default();
                    if stdout.1 || stderr.1 {
                        ContainerResult::OutputLimit
                    } else {
                        ContainerResult::Exited {
                            success: status.success(),
                            stdout: stdout.0,
                            stderr: stderr.0,
                            oom_killed: self.oom_killed(name).await,
                        }
                    }
                }
                Err(_) => ContainerResult::SystemError,
            },
            _ = tokio::time::sleep(limits.timeout) => {
                let _ = child.kill().await;
                ContainerResult::Timeout
            },
            Some(_) = overflow_rx.recv() => {
                let _ = child.kill().await;
                ContainerResult::OutputLimit
            }
        };
        self.cleanup(name).await;
        Ok(result)
    }

    pub async fn judge(
        &self,
        job: &LeasedJob,
        test_cases: &[JudgeTestCase],
    ) -> Result<JudgeOutcome, SandboxError> {
        if test_cases.is_empty() {
            return Ok(JudgeOutcome {
                verdict: Verdict::SystemError,
                score: 0,
                compile_output: None,
            });
        }
        let workspace = tempfile::Builder::new().prefix("alpha-judge-").tempdir()?;
        std::fs::set_permissions(workspace.path(), std::fs::Permissions::from_mode(0o777))?;
        let (source_name, compile_command, run_command) = language_commands(job)?;
        let source_path = workspace.path().join(source_name);
        tokio::fs::write(&source_path, job.source.as_bytes()).await?;
        std::fs::set_permissions(&source_path, std::fs::Permissions::from_mode(0o666))?;

        let compile_name = format!("alpha-{}-compile", job.job_id.simple());
        let compile = self
            .execute_container(
                &compile_name,
                workspace.path(),
                &compile_command,
                &[],
                ExecutionLimits {
                    memory_mb: 1024,
                    timeout: Duration::from_secs(30),
                    stdout_bytes: COMPILE_OUTPUT_LIMIT,
                    stderr_bytes: COMPILE_OUTPUT_LIMIT,
                },
            )
            .await?;
        match compile {
            ContainerResult::Exited { success: true, .. } => {}
            ContainerResult::Exited { stderr, .. } => {
                return Ok(JudgeOutcome {
                    verdict: Verdict::CompileError,
                    score: 0,
                    compile_output: Some(String::from_utf8_lossy(&stderr).into_owned()),
                });
            }
            ContainerResult::Timeout => {
                return Ok(JudgeOutcome {
                    verdict: Verdict::CompileError,
                    score: 0,
                    compile_output: Some("컴파일 제한 시간을 초과했습니다".to_owned()),
                });
            }
            ContainerResult::OutputLimit => {
                return Ok(JudgeOutcome {
                    verdict: Verdict::CompileError,
                    score: 0,
                    compile_output: Some("컴파일 출력 제한을 초과했습니다".to_owned()),
                });
            }
            ContainerResult::SystemError => {
                return Ok(JudgeOutcome {
                    verdict: Verdict::SystemError,
                    score: 0,
                    compile_output: None,
                });
            }
        }

        let checker = match job.checker_kind.as_str() {
            "exact" => Checker::Exact,
            "whitespace" => Checker::Whitespace,
            "float" => Checker::Float {
                tolerance: job.float_tolerance.unwrap_or(1e-6),
            },
            _ => {
                return Ok(JudgeOutcome {
                    verdict: Verdict::SystemError,
                    score: 0,
                    compile_output: None,
                });
            }
        };
        let mut groups: BTreeMap<&str, (i32, bool)> = BTreeMap::new();
        for test_case in test_cases {
            let entry = groups.entry(&test_case.group_key).or_insert((0, true));
            entry.0 += test_case.score_weight;
            let run_name = format!("alpha-{}-run-{}", job.job_id.simple(), test_case.ordinal);
            let execution = self
                .execute_container(
                    &run_name,
                    workspace.path(),
                    &run_command,
                    test_case.input.as_bytes(),
                    ExecutionLimits {
                        memory_mb: job.memory_limit_mb,
                        timeout: Duration::from_millis(
                            u64::try_from(job.time_limit_ms).unwrap_or(30_000) + 250,
                        ),
                        stdout_bytes: STDOUT_LIMIT,
                        stderr_bytes: STDERR_LIMIT,
                    },
                )
                .await?;
            match execution {
                ContainerResult::Timeout => {
                    return Ok(JudgeOutcome {
                        verdict: Verdict::TimeLimitExceeded,
                        score: 0,
                        compile_output: None,
                    });
                }
                ContainerResult::OutputLimit => {
                    return Ok(JudgeOutcome {
                        verdict: Verdict::OutputLimitExceeded,
                        score: 0,
                        compile_output: None,
                    });
                }
                ContainerResult::Exited {
                    oom_killed: true, ..
                } => {
                    return Ok(JudgeOutcome {
                        verdict: Verdict::MemoryLimitExceeded,
                        score: 0,
                        compile_output: None,
                    });
                }
                ContainerResult::Exited { success: false, .. } => {
                    return Ok(JudgeOutcome {
                        verdict: Verdict::RuntimeError,
                        score: 0,
                        compile_output: None,
                    });
                }
                ContainerResult::Exited { stdout, .. } => {
                    if !checker.accepts(
                        &String::from_utf8_lossy(&stdout),
                        &test_case.expected_output,
                    ) {
                        entry.1 = false;
                    }
                }
                ContainerResult::SystemError => {
                    return Ok(JudgeOutcome {
                        verdict: Verdict::SystemError,
                        score: 0,
                        compile_output: None,
                    });
                }
            }
        }
        let total_weight: i32 = groups.values().map(|(weight, _)| weight).sum();
        let passed_weight: i32 = groups
            .values()
            .filter(|(_, passed)| *passed)
            .map(|(weight, _)| weight)
            .sum();
        let score = i16::try_from((passed_weight * 100) / total_weight.max(1)).unwrap_or(0);
        let verdict = if score == 100 {
            Verdict::Accepted
        } else if score > 0 {
            Verdict::PartialAccepted
        } else {
            Verdict::WrongAnswer
        };
        Ok(JudgeOutcome {
            verdict,
            score,
            compile_output: None,
        })
    }
}

fn language_commands(
    job: &LeasedJob,
) -> Result<(&'static str, Vec<String>, Vec<String>), SandboxError> {
    match job.language.as_str() {
        "cpp20" => Ok((
            "Main.cpp",
            [
                "/usr/bin/g++",
                "-std=c++20",
                "-O2",
                "-pipe",
                "-o",
                "/workspace/Main",
                "/workspace/Main.cpp",
            ]
            .map(str::to_owned)
            .to_vec(),
            vec!["/workspace/Main".to_owned()],
        )),
        "python3" => Ok((
            "Main.py",
            ["/usr/bin/python3", "-m", "py_compile", "/workspace/Main.py"]
                .map(str::to_owned)
                .to_vec(),
            ["/usr/bin/python3", "-I", "/workspace/Main.py"]
                .map(str::to_owned)
                .to_vec(),
        )),
        "java21" => {
            let heap_mb = (job.memory_limit_mb - 64).clamp(32, 1984);
            Ok((
                "Main.java",
                [
                    "/usr/bin/javac",
                    "-encoding",
                    "UTF-8",
                    "/workspace/Main.java",
                ]
                .map(str::to_owned)
                .to_vec(),
                vec![
                    "/usr/bin/java".to_owned(),
                    format!("-Xmx{heap_mb}m"),
                    "-cp".to_owned(),
                    "/workspace".to_owned(),
                    "Main".to_owned(),
                ],
            ))
        }
        _ => Err(SandboxError::InvalidConfiguration),
    }
}
