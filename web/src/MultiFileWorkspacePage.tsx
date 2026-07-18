import { useEffect, useRef, useState } from 'react';
import PortalHeader from './PortalHeader';

type FileEntry = { path: string; content: string };
type Workspace = { id: string; title: string; version: number; files: FileEntry[] };
type Run = { id: string; status: string; stdout?: string; stderr?: string };
const csrf = () => document.cookie.split(';').map((value) => value.trim()).find((value) => value.startsWith('alpha_csrf='))?.slice(11) ?? '';
const statusLabels: Record<string, string> = { queued: '실행 대기', running: '실행 중', succeeded: '통과', failed: '실패', cancelled: '취소됨' };

function MultiFileWorkspacePage() {
  const requestedId = new URLSearchParams(window.location.search).get('id');
  const [workspace, setWorkspace] = useState<Workspace | null>(null);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [activePath, setActivePath] = useState('');
  const [title, setTitle] = useState('');
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
    if (!run || !['queued', 'running'].includes(run.status)) return;
    const timer = window.setInterval(() => {
      fetch(`/api/v1/workspace-runs/${run.id}`, { credentials: 'include' })
        .then((response) => response.json()).then(setRun).catch(() => undefined);
    }, 2000);
    return () => window.clearInterval(timer);
  }, [run?.id, run?.status]);

  const createWorkspace = async () => {
    const payload = { title: title.trim(), template_slug: 'python-cli-v1', project_id: null, files: [{ path: 'main.py', content: 'print("안녕하세요")', symlink: false }] };
    const response = await fetch('/api/v1/workspaces', { method: 'POST', credentials: 'include', headers, body: JSON.stringify({ ...payload, idempotency_key: keyFor('create', payload) }) });
    const body = await response.json();
    if (!response.ok) return setMessage(body.error?.message ?? '작업공간을 만들지 못했습니다.');
    setWorkspace(body); setActivePath(body.files?.[0]?.path ?? '');
  };
  const updateFile = (content: string) => setWorkspace((current) => current && ({ ...current, files: current.files.map((file) => file.path === activePath ? { ...file, content } : file) }));
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
    if (response.ok) setRun({ ...run, status: 'cancelled' }); else setMessage(body.error?.message ?? '취소하지 못했습니다.');
  };
  const activeFile = workspace?.files.find((file) => file.path === activePath);

  return <div className="learning-page"><PortalHeader /><main className="path-panel workspace-panel">
    <p className="eyebrow">격리된 다중 파일 실행</p><h1>프로젝트 작업공간</h1>
    {!workspace && <section className="project-form" aria-label="작업공간 만들기"><label>작업공간 제목<input value={title} onChange={(event) => setTitle(event.target.value)} /></label><button type="button" disabled={!title.trim()} onClick={createWorkspace}>CLI 작업공간 만들기</button>{workspaces.map((item) => <a key={item.id} href={`/workspace?id=${item.id}`}>{item.title}</a>)}</section>}
    {workspace && <><h2>{workspace.title}</h2><div className="workspace-tabs" role="tablist" aria-label="프로젝트 파일">{workspace.files.map((file) => <button role="tab" aria-selected={file.path === activePath} type="button" key={file.path} onClick={() => setActivePath(file.path)}>{file.path}</button>)}</div>
      {activeFile && <label className="workspace-editor">{activeFile.path} 내용<textarea rows={16} value={activeFile.content} onChange={(event) => updateFile(event.target.value)} /></label>}
      <div className="workspace-actions"><button type="button" onClick={save}>파일 저장</button><button type="button" onClick={execute}>빌드 및 테스트 실행</button>{run && ['queued', 'running'].includes(run.status) && <button type="button" onClick={cancel}>실행 취소</button>}</div>
      {run && <section className="run-result" aria-live="polite"><strong>{statusLabels[run.status] ?? run.status}</strong>{run.stdout && <pre aria-label="표준 출력">{run.stdout}</pre>}{run.stderr && <pre aria-label="오류 출력">{run.stderr}</pre>}</section>}</>}
    {message && <p role="status" className="auth-message">{message}</p>}
  </main></div>;
}

export default MultiFileWorkspacePage;
