import { FormEvent, useRef, useState } from 'react';
import PortalHeader from './PortalHeader';

type Idea = { id: string; title: string; features: string[]; milestones: Array<{ position: number; title: string; estimated_minutes: number;visible_result:boolean }>; scope_reduced: boolean;feasibility_reasons:string[];excluded_features:Array<{feature:string;reason_code:string}> };
const csrf = () => document.cookie.split(';').map((v) => v.trim()).find((v) => v.startsWith('alpha_csrf='))?.slice(11) ?? '';

function ProjectIdeationPage() {
  const [idea, setIdea] = useState<Idea | null>(null); const [message, setMessage] = useState('');
  const [title, setTitle] = useState('나만의 학습 기록 앱'); const [features, setFeatures] = useState('기록,목록,통계,친구,랭킹,채팅');
  const ideaKey = useRef(crypto.randomUUID()); const projectKey = useRef(crypto.randomUUID());
  const confirmKey=useRef(crypto.randomUUID());const[confirmed,setConfirmed]=useState(false);
  const submit = async (event: FormEvent) => { event.preventDefault();
    const response = await fetch('/api/v1/projects/ideas', { method: 'POST', credentials: 'include', headers: { 'content-type': 'application/json', 'x-csrf-token': csrf() }, body: JSON.stringify({ title, motivation: '학습 과정을 직접 확인하고 싶음', target_user: '혼자 공부하는 학습자', intended_outcome: '오늘의 학습 기록 확인', core_feature: '학습 기록', technology: 'typescript', weekly_minutes: 120, requested_features: features.split(',').map((v) => v.trim()).filter(Boolean), assistance_policy: 'documentation_navigator',skill_level:'beginner',runtime:'browser',infrastructure:'local_only',source_kind:'original',repository_url:null,repository_revision:null,ownership_basis:null,license_identifier:null, idempotency_key: ideaKey.current }) });
    const body = await response.json(); if (!response.ok) { setMessage(body.error?.message ?? '아이디어를 만들지 못했습니다.'); return; } setIdea(body);setConfirmed(false);projectKey.current=crypto.randomUUID();confirmKey.current=crypto.randomUUID();
  };
  const confirm=async()=>{if(!idea)return;const response=await fetch(`/api/v1/projects/ideas/${idea.id}/confirm`,{method:'POST',credentials:'include',headers:{'content-type':'application/json','x-csrf-token':csrf()},body:JSON.stringify({scoped_features:idea.features,milestones:idea.milestones,idempotency_key:confirmKey.current})});if(response.ok)setConfirmed(true);else setMessage('범위를 확정하지 못했습니다.');};
  const start = async () => { if (!idea) return; const response = await fetch('/api/v1/projects', { method: 'POST', credentials: 'include', headers: { 'content-type': 'application/json', 'x-csrf-token': csrf() }, body: JSON.stringify({ idea_id: idea.id, idempotency_key: projectKey.current }) }); const body = await response.json(); if (response.ok) window.location.assign(`/projects/workspace?id=${body.id}`); else setMessage(body.error?.message ?? '프로젝트를 시작하지 못했습니다.'); };
  return <div className="learning-page"><PortalHeader /><main className="path-panel project-learning-panel"><p className="eyebrow">프로젝트 아이디어 스튜디오</p><h1>관심을 작은 결과로</h1>
    <form className="project-form" onSubmit={submit}><label>프로젝트 이름<input value={title} maxLength={120} onChange={(e) => { setTitle(e.target.value);setIdea(null);setConfirmed(false); ideaKey.current = crypto.randomUUID();projectKey.current=crypto.randomUUID(); }} required /></label><label>원하는 기능(쉼표로 구분)<textarea value={features} maxLength={1000} onChange={(e) => { setFeatures(e.target.value);setIdea(null);setConfirmed(false); ideaKey.current = crypto.randomUUID();projectKey.current=crypto.randomUUID(); }} required /></label><button className="primary-action" type="submit">실행 가능한 범위 확인</button></form>
    {idea && <section className="project-result" aria-live="polite"><p className="status-dot">AI 없이 결정론적 범위 검사 완료</p><h2>{idea.title}</h2>{idea.scope_reduced && <p>첫 버전에 필요한 기능으로 줄였습니다. 최종 선택은 학습자가 확정합니다.</p>}<p>판정 이유: {idea.feasibility_reasons.join(' · ')}</p>{idea.excluded_features.map((item)=><p key={item.feature}>{item.feature} 제외 · {item.reason_code}</p>)}<ul>{idea.features.map((feature) => <li key={feature}>{feature}</li>)}</ul><ol>{idea.milestones.map((m) => <li key={m.position}><strong>{m.title}</strong><span>{m.estimated_minutes}분 · 보이는 결과</span></li>)}</ol>{!confirmed?<button type="button" onClick={confirm}>이 범위 확정</button>:<button type="button" onClick={start}>이 프로젝트 선택</button>}</section>}
    {message && <p role="alert" className="auth-message">{message}</p>}</main></div>;
}
export default ProjectIdeationPage;
