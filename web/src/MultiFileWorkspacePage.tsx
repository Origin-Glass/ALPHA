import { useEffect, useRef, useState } from 'react';
import PortalHeader from './PortalHeader';

type FileEntry = { path: string; content: string };
type Workspace = { id: string; title: string; version: number; template_slug?: string; runtime_status?: string; files: FileEntry[] };
type Run = { id: string; status: string; stdout?: string; stderr?: string; cancel_requested?: boolean; preview_html?: string };
const csrf = () => document.cookie.split(';').map((value) => value.trim()).find((value) => value.startsWith('alpha_csrf='))?.slice(11) ?? '';
const statusLabels: Record<string, string> = { queued: '실행 대기', leased: '실행 준비 중', running: '실행 중', succeeded: '통과', failed: '실패', cancelled: '취소됨', expired: '만료됨' };

function MultiFileWorkspacePage() {
  const requestedId = new URLSearchParams(window.location.search).get('id');
  const [workspace, setWorkspace] = useState<Workspace | null>(null);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [activePath, setActivePath] = useState('');
  const [title, setTitle] = useState('');
  const [template, setTemplate] = useState('python-cli-v1');
  const [newPath, setNewPath] = useState('');
  const [resetVersion, setResetVersion] = useState('1');
  const [diff, setDiff] = useState<string[]>([]);
  const [run, setRun] = useState<Run | null>(null);
  const [message, setMessage] = useState('');
  const keys = useRef<Record<string, string>>({});
  const keyFor = (action: string, payload: unknown) => {
    const signature = `${action}:${JSON.stringify(payload)}`;
    return keys.current[signature] ??= crypto.randomUUID();
  };
  const headers = { 'content-type': 'application/json', 'x-csrf-token': csrf() };

  useEffect(() => {
    const path = requestedId ? `/api/v1/workspaces/${requestedId}` : '/api/v1/workspaces';
    fetch(path, { credentials: 'include' }).then(async (response) => {
      const body = await response.json();
      if (!response.ok) throw new Error(body.error?.message ?? '작업공간을 불러오지 못했습니다.');
      if (requestedId) {
        setWorkspace(body);
        setActivePath(body.files?.[0]?.path ?? '');
      } else setWorkspaces(body.workspaces ?? []);
    }).catch((error) => setMessage(error.message));
  }, [requestedId]);

  useEffect(() => {
    if (!run || !['queued', 'leased', 'running'].includes(run.status)) return;
    const timer = window.setInterval(() => {
      fetch(`/api/v1/workspace-runs/${run.id}`, { credentials: 'include' })
        .then((response) => response.json()).then(setRun).catch(() => undefined);
    }, 2000);
    return () => window.clearInterval(timer);
  }, [run?.id, run?.status]);

  const createWorkspace = async () => {
    const starters: Record<string, Array<FileEntry & { symlink: boolean }>> = {
      'python-cli-v1': [{ path: 'main.py', content: 'print("안녕하세요")', symlink: false }],
      'python-test-v1': [{ path: 'test_project.py', content: 'import unittest\n\nclass ProjectTest(unittest.TestCase):\n    def test_example(self):\n        self.assertTrue(True)\n', symlink: false }],
      'python-api-v1': [
        { path: 'app.py', content: 'import json\nfrom http.server import BaseHTTPRequestHandler\n\nclass Handler(BaseHTTPRequestHandler):\n    def do_GET(self):\n        if self.path != "/health":\n            self.send_error(404)\n            return\n        body = json.dumps({"status": "ok"}).encode()\n        self.send_response(200)\n        self.send_header("Content-Type", "application/json")\n        self.send_header("Content-Length", str(len(body)))\n        self.end_headers()\n        self.wfile.write(body)\n    def log_message(self, *_):\n        pass\n', symlink: false },
        { path: 'test_app.py', content: 'import json\nimport threading\nimport unittest\nimport urllib.request\nfrom http.server import ThreadingHTTPServer\nfrom app import Handler\n\nclass ApiTest(unittest.TestCase):\n    def test_loopback_request_response(self):\n        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)\n        thread = threading.Thread(target=server.serve_forever, daemon=True)\n        thread.start()\n        try:\n            with urllib.request.urlopen(f"http://127.0.0.1:{server.server_port}/health") as response:\n                self.assertEqual(response.status, 200)\n                self.assertEqual(json.load(response), {"status": "ok"})\n        finally:\n            server.shutdown()\n            server.server_close()\n            thread.join()\n', symlink: false },
      ],
      'static-web-v1': [{ path: 'index.html', content: '<!doctype html><html lang="ko"><head><meta charset="utf-8"><title>정적 웹</title></head><body><button id="hello">눌러 보세요</button><script>hello.onclick=()=>hello.textContent="안녕하세요"</script></body></html>', symlink: false }],
    };
    const starter = starters[template] ?? starters['python-cli-v1'];
    const payload = { title: title.trim(), template_slug: template, project_id: null, files: starter };
    const response = await fetch('/api/v1/workspaces', { method: 'POST', credentials: 'include', headers, body: JSON.stringify({ ...payload, idempotency_key: keyFor('create', payload) }) });
    const body = await response.json();
    if (!response.ok) return setMessage(body.error?.message ?? '작업공간을 만들지 못했습니다.');
    setWorkspace(body); setActivePath(body.files?.[0]?.path ?? '');
  };
  const updateFile = (content: string) => setWorkspace((current) => current && ({ ...current, files: current.files.map((file) => file.path === activePath ? { ...file, content } : file) }));
  const addFile = () => { const path = newPath.trim(); if (!workspace || !path || workspace.files.some((file) => file.path.toLowerCase() === path.toLowerCase())) return; setWorkspace({ ...workspace, files: [...workspace.files, { path, content: '' }] }); setActivePath(path); setNewPath(''); };
  const renameFile = () => { const path = newPath.trim(); if (!workspace || !activeFile || !path || workspace.files.some((file) => file.path.toLowerCase() === path.toLowerCase())) return; setWorkspace({ ...workspace, files: workspace.files.map((file) => file.path === activePath ? { ...file, path } : file) }); setActivePath(path); setNewPath(''); };
  const deleteFile = () => { if (!workspace || workspace.files.length <= 1) return setMessage('작업공간에는 파일이 하나 이상 필요합니다.'); const files = workspace.files.filter((file) => file.path !== activePath); setWorkspace({ ...workspace, files }); setActivePath(files[0].path); };
  const save = async () => {
    if (!workspace) return;
    const payload = { files: workspace.files, expected_version: workspace.version };
    const response = await fetch(`/api/v1/workspaces/${workspace.id}`, { method: 'PUT', credentials: 'include', headers, body: JSON.stringify({ ...payload, idempotency_key: keyFor('save', payload) }) });
    const body = await response.json();
    if (response.ok) { setWorkspace(body); setMessage('파일을 저장했습니다.'); } else setMessage(body.error?.message ?? '저장하지 못했습니다.');
  };
  const execute = async () => {
    if (!workspace) return;
    const payload = { workspace_version: workspace.version };
    const response = await fetch(`/api/v1/workspaces/${workspace.id}/runs`, { method: 'POST', credentials: 'include', headers, body: JSON.stringify({ expected_version: workspace.version, idempotency_key: keyFor('run', payload) }) });
    const body = await response.json();
    if (response.ok) setRun(body); else setMessage(body.error?.message ?? '실행하지 못했습니다.');
  };
  const cancel = async () => {
    if (!run) return;
    const response = await fetch(`/api/v1/workspace-runs/${run.id}/cancel`, { method: 'POST', credentials: 'include', headers, body: JSON.stringify({ idempotency_key: keyFor('cancel', run.id) }) });
    const body = await response.json();
    if (response.ok) setRun({ ...run, cancel_requested: true }); else setMessage(body.error?.message ?? '취소하지 못했습니다.');
  };
  const checkpoint = async () => { if (!workspace) return; const payload = { name: `버전 ${workspace.version}` }; const response = await fetch(`/api/v1/workspaces/${workspace.id}/checkpoints`, { method: 'POST', credentials: 'include', headers, body: JSON.stringify({ ...payload, idempotency_key: keyFor('checkpoint', payload) }) }); setMessage(response.ok ? '체크포인트를 만들었습니다.' : '체크포인트를 만들지 못했습니다.'); };
  const showDiff = async () => { if (!workspace) return; const response = await fetch(`/api/v1/workspaces/${workspace.id}/revisions/${workspace.version}`, { credentials: 'include' }); const body = await response.json(); if (response.ok) setDiff(body.changed_paths ?? []); else setMessage(body.error?.message ?? '변경 내역을 불러오지 못했습니다.'); };
  const reset = async () => { if (!workspace) return; const payload = { version: Number(resetVersion), expected_version: workspace.version }; const response = await fetch(`/api/v1/workspaces/${workspace.id}/reset`, { method: 'POST', credentials: 'include', headers, body: JSON.stringify({ ...payload, idempotency_key: keyFor('reset', payload) }) }); const body = await response.json(); if (response.ok) { setWorkspace(body); setActivePath(body.files[0]?.path ?? ''); } else setMessage(body.error?.message ?? '리비전을 복원하지 못했습니다.'); };
  const activeFile = workspace?.files.find((file) => file.path === activePath);

  return <div className="learning-page"><PortalHeader /><main className="path-panel workspace-panel">
    <p className="eyebrow">격리된 다중 파일 실행</p><h1>프로젝트 작업공간</h1>
    {!workspace && <section className="project-form" aria-label="작업공간 만들기"><label>작업공간 제목<input value={title} onChange={(event) => setTitle(event.target.value)} /></label><label>실행 템플릿<select value={template} onChange={(event) => setTemplate(event.target.value)}><option value="python-cli-v1">Python CLI</option><option value="python-test-v1">Python unittest</option><option value="python-api-v1">Python loopback API</option><option value="static-web-v1">정적 웹</option><option value="rust-cli-v1" disabled>Rust CLI — 런타임 미검증</option></select></label><button type="button" disabled={!title.trim()} onClick={createWorkspace}>작업공간 만들기</button>{workspaces.map((item) => <a key={item.id} href={`/workspace?id=${item.id}`}>{item.title}</a>)}</section>}
    {workspace && <><h2>{workspace.title}</h2>{workspace.runtime_status && workspace.runtime_status !== 'verified' && <p role="alert">이 템플릿은 런타임 미검증 상태라 실행할 수 없습니다.</p>}<div className="workspace-tabs" role="tablist" aria-label="프로젝트 파일">{workspace.files.map((file) => <button role="tab" aria-selected={file.path === activePath} type="button" key={file.path} onClick={() => setActivePath(file.path)}>{file.path}</button>)}</div>
      {activeFile && <label className="workspace-editor">{activeFile.path} 내용<textarea rows={16} value={activeFile.content} onChange={(event) => updateFile(event.target.value)} /></label>}
      <div className="workspace-actions"><label>파일 경로<input value={newPath} onChange={(event) => setNewPath(event.target.value)} /></label><button type="button" onClick={addFile}>파일 추가</button><button type="button" onClick={renameFile}>선택 파일 이름 변경</button><button type="button" onClick={deleteFile}>선택 파일 삭제</button><button type="button" onClick={save}>파일 저장</button><button type="button" disabled={workspace.runtime_status !== undefined && workspace.runtime_status !== 'verified'} onClick={execute}>빌드 및 테스트 실행</button>{run && ['queued', 'leased', 'running'].includes(run.status) && !run.cancel_requested && <button type="button" onClick={cancel}>실행 취소</button>}</div>
      <div className="workspace-actions"><button type="button" onClick={checkpoint}>체크포인트 만들기</button><button type="button" onClick={showDiff}>현재 변경 내역</button><label>복원할 버전<input type="number" min="1" value={resetVersion} onChange={(event) => setResetVersion(event.target.value)} /></label><button type="button" onClick={reset}>리비전 복원</button></div>{diff.length > 0 && <p>변경 파일: {diff.join(', ')}</p>}
      {run && <section className="run-result" aria-live="polite"><strong>{run.cancel_requested && !['cancelled', 'expired'].includes(run.status) ? '취소 요청됨' : statusLabels[run.status] ?? run.status}</strong>{run.stdout && <pre aria-label="표준 출력">{run.stdout}</pre>}{run.stderr && <pre aria-label="오류 출력">{run.stderr}</pre>}{run.preview_html && <iframe title="서명된 정적 미리보기" sandbox="allow-scripts" srcDoc={`<meta http-equiv="Content-Security-Policy" content="default-src 'none'; connect-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'">${run.preview_html}`} />}</section>}</>}
    {message && <p role="status" className="auth-message">{message}</p>}
  </main></div>;
}

export default MultiFileWorkspacePage;
