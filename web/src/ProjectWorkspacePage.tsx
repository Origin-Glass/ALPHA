import { useEffect, useState } from 'react';
import PortalHeader from './PortalHeader';
type Project = { id: string; title: string; technology: string; assistance_policy: string; milestones: Array<{ id: string; position: number; title: string; estimated_minutes: number; status: string }> };
const csrf = () => document.cookie.split(';').map((v) => v.trim()).find((v) => v.startsWith('alpha_csrf='))?.slice(11) ?? '';

function ProjectWorkspacePage() {
  const id = new URLSearchParams(window.location.search).get('id'); const [project, setProject] = useState<Project | null>(null); const [help, setHelp] = useState('');
  useEffect(() => { if (!id) return; fetch(`/api/v1/projects/${id}`, { credentials: 'include' }).then(async (r) => { const b = await r.json(); if (!r.ok) throw new Error(b.error?.message); return b; }).then(setProject).catch(() => setHelp('프로젝트를 불러오지 못했습니다.')); }, [id]);
  const requestHelp = async (milestone: Project['milestones'][number]) => { if (!project) return; const response = await fetch(`/api/v1/projects/${project.id}/assistance`, { method: 'POST', credentials: 'include', headers: { 'content-type': 'application/json', 'x-csrf-token': csrf() }, body: JSON.stringify({ milestone_id: milestone.id, skill: project.technology }) }); const body = await response.json(); setHelp(response.ok ? `${body.content} (도움 단계 ${body.level}, AI 사용 안 함)` : body.error?.message); };
  return <div className="learning-page"><PortalHeader /><main className="path-panel project-learning-panel"><p className="eyebrow">프로젝트 작업 공간</p><h1>{project?.title ?? '프로젝트 불러오는 중'}</h1>{project && <><p>도움 정책: {project.assistance_policy} · 기술: {project.technology}</p><ol className="unit-list">{project.milestones.map((m) => <li className="unit-card" key={m.id}><div className="unit-position">{String(m.position).padStart(2, '0')}</div><div><h2>{m.title}</h2><p>{m.estimated_minutes}분 안에 확인 가능한 결과를 만듭니다.</p><button type="button" onClick={() => requestHelp(m)}>다음 도움 요청</button></div><span className="unit-state">{m.status}</span></li>)}</ol></>}{help && <p role="status" className="auth-message">{help}</p>}</main></div>;
}
export default ProjectWorkspacePage;
