use alpha::{sandbox::DockerSandbox, workspaces::WorkspaceFileInput};
use tokio::process::Command;
use uuid::Uuid;

fn file(path: &str, content: &str) -> WorkspaceFileInput {
    WorkspaceFileInput::new(path, content, false)
}
async fn absent(run: Uuid) {
    let output = Command::new("docker")
        .args([
            "container",
            "inspect",
            &format!("alpha-workspace-{}", run.simple()),
        ])
        .output()
        .await
        .unwrap();
    assert!(
        !output.status.success(),
        "완료된 컨테이너가 정리되지 않았습니다"
    );
}

#[tokio::test]
#[ignore = "Docker 격리 런타임이 있는 CI에서 별도 실행"]
async fn multi_file_workspace_enforces_real_isolation_limits_and_cleanup() {
    let image=std::env::var("WORKSPACE_TEST_IMAGE").unwrap_or_else(|_|"alpha-judge-runner@sha256:e0a1147badcf2997c64f1cc3058d015ea0cf6865511d09e2f1506f06377d89de".into());
    assert!(image.contains("@sha256:"));
    let sandbox = DockerSandbox::new("docker".into(), image).unwrap();
    sandbox.verify().await.unwrap();
    let image_id = sandbox.immutable_image_id().await.unwrap();
    assert!(image_id.starts_with("sha256:") && image_id.len() == 71);
    let run = Uuid::now_v7();
    let files = vec![
        file(
            "main.py",
            r#"import os,socket,helper
assert os.getuid()!=0
blocked=0
try: open('/root/escape','w').write('x')
except OSError: blocked+=1
try: socket.create_connection(('1.1.1.1',80),.2)
except OSError: blocked+=1
print(helper.answer() if blocked==2 else 'isolation-bypass')"#,
        ),
        file("helper.py", "def answer(): return 42"),
    ];
    let out = sandbox
        .run_workspace(run, &files, &["python3".into(), "main.py".into()])
        .await
        .unwrap();
    assert_eq!(out.status, "succeeded");
    assert_eq!(out.stdout, "42\n");
    absent(run).await;
    let run = Uuid::now_v7();
    let out = sandbox
        .run_workspace(
            run,
            &[
                file(
                    "test_project.py",
                    "import unittest\nimport helper\n\nclass ProjectTest(unittest.TestCase):\n    def test_uses_project_module(self):\n        self.assertEqual(helper.answer(), 42)\n",
                ),
                file("helper.py", "def answer(): return 42"),
            ],
            &[
                "sh".into(),
                "-lc".into(),
                "PYTHONDONTWRITEBYTECODE=1 python3 -m unittest -v".into(),
            ],
        )
        .await
        .unwrap();
    assert_eq!(out.status, "succeeded", "{}", out.stderr);
    assert!(out.stderr.contains("OK"));
    absent(run).await;
    let run = Uuid::now_v7();
    let out = sandbox
        .run_workspace(
            run,
            &[
                file("app.py", "import json\nfrom http.server import BaseHTTPRequestHandler\nclass Handler(BaseHTTPRequestHandler):\n def do_GET(self):\n  body=json.dumps({'status':'ok'}).encode(); self.send_response(200); self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)\n def log_message(self,*_): pass\n"),
                file("test_app.py", "import json,threading,unittest,urllib.request\nfrom http.server import ThreadingHTTPServer\nfrom app import Handler\nclass ApiTest(unittest.TestCase):\n def test_loopback(self):\n  server=ThreadingHTTPServer(('127.0.0.1',0),Handler); thread=threading.Thread(target=server.serve_forever,daemon=True); thread.start()\n  try:\n   with urllib.request.urlopen(f'http://127.0.0.1:{server.server_port}/health') as response:\n    self.assertEqual(response.status,200); self.assertEqual(json.load(response),{'status':'ok'})\n  finally:\n   server.shutdown(); server.server_close(); thread.join()\n"),
            ],
            &["sh".into(), "-lc".into(), "PYTHONDONTWRITEBYTECODE=1 python3 -m unittest -v".into()],
        )
        .await
        .unwrap();
    assert_eq!(out.status, "succeeded", "{}", out.stderr);
    assert!(out.stderr.contains("test_loopback") && out.stderr.contains("OK"));
    absent(run).await;
    let run = Uuid::now_v7();
    let out = sandbox.run_workspace(run,&[file("index.html","<!doctype html><html><body><button>안녕</button></body></html>")],&["python3".into(),"-c".into(),"s=open('index.html',encoding='utf-8').read().lower();assert '<html' in s and '<body' in s and '</body>' in s".into()]).await.unwrap();
    assert_eq!(out.status, "succeeded", "{}", out.stderr);
    absent(run).await;
    let run = Uuid::now_v7();
    let out = sandbox
        .run_workspace(
            run,
            &[file("main.py", "while True: print('x'*8192)")],
            &["python3".into(), "main.py".into()],
        )
        .await
        .unwrap();
    assert_eq!(out.status, "failed");
    assert!(out.output_truncated);
    absent(run).await;
    let run = Uuid::now_v7();
    let out = sandbox
        .run_workspace(
            run,
            &[file("main.py", "while True: pass")],
            &["python3".into(), "main.py".into()],
        )
        .await
        .unwrap();
    assert_eq!(out.status, "failed");
    assert!(out.stderr.contains("시간"));
    absent(run).await;
    let run = Uuid::now_v7();
    let out = sandbox
        .run_workspace(
            run,
            &[file(
                "main.py",
                "x=[]\nwhile True: x.append(bytearray(16*1024*1024))",
            )],
            &["python3".into(), "main.py".into()],
        )
        .await
        .unwrap();
    assert_eq!(out.status, "failed");
    absent(run).await;
    let run = Uuid::now_v7();
    let out = sandbox
        .run_workspace(
            run,
            &[file(
                "main.py",
                r#"import os
p=[]
for _ in range(200):
 try:
  pid=os.fork()
  if pid==0: os._exit(0)
  p.append(pid)
 except OSError: break
for pid in p: os.waitpid(pid,0)
print('limited' if len(p)<200 else 'unlimited')"#,
            )],
            &["python3".into(), "main.py".into()],
        )
        .await
        .unwrap();
    assert_eq!(out.status, "succeeded");
    assert_eq!(out.stdout, "limited\n");
    absent(run).await;
}
