import { FormEvent, useEffect, useRef, useState } from 'react';
import PortalHeader from './PortalHeader';

type Plan = { id:string; revision:number; target_outcome: string; recommendation_key: string; reason_codes: string[]; items: Array<{ kind: string; title: string; estimated_minutes: number }>; provider_used: boolean; rule_version: string; restored_from_id:string|null; deadline:string;preferred_framework:string;privacy:string };
const csrf = () => document.cookie.split(';').map((v) => v.trim()).find((v) => v.startsWith('alpha_csrf='))?.slice(11) ?? '';
const axisLabels:Record<string,string>={algorithmic_reasoning:'알고리즘 추론 진단',code_literacy:'코드 이해 진단',docs_learning:'문서 학습 진단',independent_coding:'독립 구현 진단'};

function LearningPlanBuilderPage() {
  const [plan, setPlan] = useState<Plan | null>(null);
  const [message, setMessage] = useState('');
  const [target, setTarget] = useState('작동하는 한국어 학습 기록 프로젝트 완성');
  const [minutes, setMinutes] = useState(180);
  const [language,setLanguage]=useState('typescript');const[pathMode,setPathMode]=useState('structured');
  const [interests,setInterests]=useState('웹,학습 기록');const[goals,setGoals]=useState('독립 구현');
  const [diagnostics,setDiagnostics]=useState({algorithmic_reasoning:50,code_literacy:50,docs_learning:50,independent_coding:50});
  const [deadline,setDeadline]=useState('2026-12-31');const[framework,setFramework]=useState('react');
  const [desiredProject,setDesiredProject]=useState('한국어 학습 기록 앱');const[curriculum,setCurriculum]=useState('code-reading,debugging');
  const [constraints,setConstraints]=useState('');const[checkpoints,setCheckpoints]=useState('첫 작동 결과,독립 전이');
  const [assistancePolicy,setAssistancePolicy]=useState('documentation_navigator');const[privacy,setPrivacy]=useState('private');
  const planKey = useRef(crypto.randomUUID());
  const rejectKey = useRef(crypto.randomUUID());
  const [history,setHistory]=useState<Plan[]>([]); const restoreKeys=useRef<Record<number,string>>({});
  const loadHistory=()=>fetch('/api/v1/learning/plans',{credentials:'include'}).then((r)=>r.ok?r.json():null).then((b)=>b&&setHistory(b.revisions)).catch(()=>undefined);
  useEffect(()=>{void loadHistory();},[]);
  const changed=()=>{setPlan(null);planKey.current=crypto.randomUUID();};const list=(value:string)=>value.split(',').map((v)=>v.trim()).filter(Boolean);

  const submit = async (event: FormEvent) => {
    event.preventDefault(); setMessage('');
    const response = await fetch('/api/v1/learning/plans', { method: 'POST', credentials: 'include', headers: { 'content-type': 'application/json', 'x-csrf-token': csrf() }, body: JSON.stringify({
      rule_version: 'project-learning-v1', idempotency_key: planKey.current, target_outcome: target, weekly_minutes: minutes,
      preferred_language: language, path_mode: pathMode, interests:list(interests), goals:list(goals), diagnostic_scores:diagnostics,
      deadline,preferred_framework:framework,desired_project:desiredProject,required_curriculum:list(curriculum),instructor_constraints:list(constraints),
      assessment_checkpoints:list(checkpoints),assistance_policy:assistancePolicy,privacy,origin:'learner',template_id:null,locked_requirements:[],
    }) });
    const body = await response.json();
    if (!response.ok) { setMessage(body.error?.message ?? '계획을 만들지 못했습니다.'); return; }
    setPlan(body); rejectKey.current = crypto.randomUUID(); void loadHistory();
  };

  const reject = async () => {
    if (!plan) return;
    const response = await fetch(`/api/v1/learning/recommendations/${plan.recommendation_key}/reject`, { method: 'POST', credentials: 'include', headers: { 'content-type': 'application/json', 'x-csrf-token': csrf() }, body: JSON.stringify({ reason: '다른 프로젝트 경로를 선택하고 싶음', idempotency_key: rejectKey.current }) });
    if(response.ok){planKey.current=crypto.randomUUID();setMessage('추천을 거부했습니다. 다시 계획하면 다른 경로를 제안합니다.');}else setMessage('추천 거부를 기록하지 못했습니다.');
  };
  const restore=async(revision:number)=>{restoreKeys.current[revision]??=crypto.randomUUID();const response=await fetch(`/api/v1/learning/plans/${revision}/restore`,{method:'POST',credentials:'include',headers:{'content-type':'application/json','x-csrf-token':csrf()},body:JSON.stringify({idempotency_key:restoreKeys.current[revision]})});const body=await response.json();if(response.ok){setPlan(body);setMessage(`${revision}번 계획을 새 리비전으로 복원했습니다.`);void loadHistory();}else setMessage(body.error?.message??'복원하지 못했습니다.');};

  return <div className="learning-page"><PortalHeader /><main className="path-panel project-learning-panel">
    <p className="eyebrow">설명 가능한 개인화</p><h1>내가 고르는 학습 계획</h1>
    <form className="project-form" onSubmit={submit}>
      <label>목표 결과<input value={target} maxLength={300} onChange={(e) => { setTarget(e.target.value);changed(); }} required /></label>
      <label>주간 학습 시간(분)<input type="number" min={30} max={2400} value={minutes} onChange={(e) => { setMinutes(Number(e.target.value));changed(); }} required /></label>
      <label>선호 언어<input value={language} pattern="[a-z0-9+#.\-]+" maxLength={32} onChange={(e)=>{setLanguage(e.target.value);changed();}} required /></label>
      <label>경로 방식<select value={pathMode} onChange={(e)=>{setPathMode(e.target.value);changed();}}><option value="structured">구조화 경로</option><option value="exploratory">탐색형 경로</option></select></label>
      <label>관심사(쉼표 구분)<input value={interests} onChange={(e)=>{setInterests(e.target.value);changed();}} required /></label>
      <label>학습 목표(쉼표 구분)<input value={goals} onChange={(e)=>{setGoals(e.target.value);changed();}} required /></label>
      {Object.entries(diagnostics).map(([axis,score])=><label key={axis}>{axisLabels[axis]}<input type="number" min={0} max={100} value={score} onChange={(e)=>{setDiagnostics({...diagnostics,[axis]:Number(e.target.value)});changed();}} required /></label>)}
      <label>완료 기한<input type="date" value={deadline} onChange={(e)=>{setDeadline(e.target.value);changed();}} required /></label>
      <label>선호 프레임워크<input value={framework} maxLength={64} onChange={(e)=>{setFramework(e.target.value);changed();}} required /></label>
      <label>만들고 싶은 프로젝트<input value={desiredProject} maxLength={200} onChange={(e)=>{setDesiredProject(e.target.value);changed();}} required /></label>
      <label>필수 교육과정(쉼표 구분)<input value={curriculum} onChange={(e)=>{setCurriculum(e.target.value);changed();}} required /></label>
      <label>강사 제약(쉼표 구분)<input value={constraints} onChange={(e)=>{setConstraints(e.target.value);changed();}} /></label>
      <label>평가 체크포인트(쉼표 구분)<input value={checkpoints} onChange={(e)=>{setCheckpoints(e.target.value);changed();}} required /></label>
      <label>도움 정책<select value={assistancePolicy} onChange={(e)=>{setAssistancePolicy(e.target.value);changed();}}><option value="guided_ai">단계형 AI</option><option value="socratic_ai">소크라테스식 도움</option><option value="documentation_navigator">문서 탐색 안내</option><option value="curated_documentation">선별 문서</option><option value="cheat_sheet_only">치트 시트만</option><option value="independent">독립 수행</option><option value="transfer_challenge">전이 과제</option></select></label>
      <label>공개 범위<select value={privacy} onChange={(e)=>{setPrivacy(e.target.value);changed();}}><option value="private">나만 보기</option><option value="instructors">강사와 공유</option><option value="classroom">학급과 공유</option></select></label>
      <button className="primary-action" type="submit">계획 만들기</button>
    </form>
    {plan && <section className="project-result" aria-live="polite"><p className="status-dot">규칙 {plan.rule_version} · AI 사용 {plan.provider_used ? '예' : '아니요'}</p><h2>{plan.target_outcome}</h2><p>{plan.deadline}까지 · {plan.preferred_framework} · 공개 범위 {plan.privacy}</p>
      <p>추천 이유: {plan.reason_codes.join(' · ')}</p><ol>{plan.items.map((item) => <li key={item.kind}><strong>{item.title}</strong><span>{item.estimated_minutes}분</span></li>)}</ol>
      <button type="button" onClick={reject}>이 추천 거부</button></section>}
    {message && <p role="status" className="auth-message">{message}</p>}
    {history.length>0&&<section className="project-result"><h2>계획 리비전 기록</h2><ol>{history.map((item)=><li key={item.id}><span>{item.revision} · {item.target_outcome}</span><button type="button" onClick={()=>restore(item.revision)}>이 계획 복원</button></li>)}</ol></section>}
  </main></div>;
}
export default LearningPlanBuilderPage;
