import { useEffect, useMemo, useState } from 'react';
import PortalHeader from './PortalHeader';

type Detail = { contest: { title: string; description: string; scoring_mode: string; starts_at: string; freezes_at: string | null; ends_at: string; state: string; participant_count: number }; problems: Array<{ label: string; slug: string; title: string; difficulty: number; points: number }>; registered: boolean };
type Board = { frozen: boolean; scoring_mode: string; entries: Array<{ rank: number; user_id: string; handle: string; display_name: string; solved: number; penalty: number; score: number }>; cells: Array<{ user_id: string; problem_label: string; solved: boolean; attempts: number; penalty_minutes: number; score: number; frozen_attempts: number }> };
const csrfToken = () => document.cookie.split(';').map((cookie) => cookie.trim()).find((cookie) => cookie.startsWith('alpha_csrf='))?.slice('alpha_csrf='.length) ?? '';
const dateTime = (value: string) => new Intl.DateTimeFormat('ko-KR', { dateStyle: 'long', timeStyle: 'short', timeZone: 'Asia/Seoul' }).format(new Date(value));

function ContestDetailPage({ slug }: { slug: string }) {
  const [detail, setDetail] = useState<Detail | null>(null);
  const [board, setBoard] = useState<Board | null>(null);
  const [joined, setJoined] = useState(false);
  const [joinCode, setJoinCode] = useState('');
  const [inviteGate, setInviteGate] = useState(false);
  const [message, setMessage] = useState('');
  const cellMap = useMemo(() => new Map(board?.cells.map((cell) => [`${cell.user_id}:${cell.problem_label}`, cell])), [board]);
  useEffect(() => {
    const load = async () => {
      const response = await fetch(`/api/v1/contests/${encodeURIComponent(slug)}`, { credentials: 'include' });
      const body = await response.json();
      if (response.status === 404) { setInviteGate(true); return; }
      if (!response.ok) throw new Error(body.error?.message ?? '대회를 불러오지 못했습니다.');
      setDetail(body);
      setJoined(body.registered);
      const score = await fetch(`/api/v1/contests/${encodeURIComponent(slug)}/scoreboard`, { credentials: 'include' });
      if (score.ok) setBoard(await score.json());
    };
    load().catch((error) => setMessage(error instanceof Error ? error.message : '대회를 불러오지 못했습니다.'));
    const timer = window.setInterval(() => fetch(`/api/v1/contests/${encodeURIComponent(slug)}/scoreboard`, { credentials: 'include' }).then((response) => response.ok ? response.json() : null).then((value) => value && setBoard(value)).catch(() => undefined), 5000);
    return () => window.clearInterval(timer);
  }, [slug]);
  const join = async () => {
    const session = await fetch('/api/v1/auth/me', { credentials: 'include' });
    if (session.status === 401) { window.location.assign(`/login?redirect_after=${encodeURIComponent(`/contests/${slug}`)}`); return; }
    const response = await fetch(`/api/v1/contests/${encodeURIComponent(slug)}/join`, { method: 'POST', credentials: 'include', headers: { 'content-type': 'application/json', 'x-csrf-token': csrfToken() }, body: JSON.stringify(joinCode ? { join_code: joinCode } : {}) });
    if (response.ok) {
      if (inviteGate) { window.location.reload(); return; }
      setJoined(true);
      const score = await fetch(`/api/v1/contests/${encodeURIComponent(slug)}/scoreboard`, { credentials: 'include' });
      if (score.ok) setBoard(await score.json());
    } else { const body = await response.json(); setMessage(body.error?.message ?? '참가 등록하지 못했습니다.'); }
  };
  const columns = detail ? `52px minmax(170px,1fr) repeat(${detail.problems.length},70px) 110px` : undefined;
  return <div className="learning-page"><PortalHeader /><main className="contest-detail">{detail ? <><a className="back-link" href="/contests">← 대회 목록</a><section className="contest-hero"><div><p className="eyebrow">{detail.contest.scoring_mode === 'icpc' ? 'ICPC SCOREBOARD' : 'SCORE CONTEST'}</p><h1>{detail.contest.title}</h1><p>{detail.contest.description}</p><div className="contest-time"><span>{dateTime(detail.contest.starts_at)} 시작</span><span>{dateTime(detail.contest.ends_at)} 종료</span>{detail.contest.freezes_at && <span>{dateTime(detail.contest.freezes_at)} 프리즈</span>}</div></div><button type="button" onClick={join} disabled={joined || detail.contest.state === 'ended'}>{joined ? '참가 등록됨' : detail.contest.state === 'ended' ? '종료됨' : '참가 등록'}</button></section><section className="contest-problems"><h2>문제</h2>{detail.problems.map((problem) => <a href={`/solve/${problem.slug}?contest=${encodeURIComponent(slug)}`} key={problem.slug}><strong>{problem.label}</strong><div><span>{problem.title}</span><small>ALPHA {problem.difficulty} · {problem.points}점</small></div><b>풀기 →</b></a>)}</section><section className="scoreboard-section"><div><h2>점수판</h2>{board?.frozen && <span className="freeze-badge">프리즈 · 이후 결과 숨김</span>}</div>{board && <div className="scoreboard-table" role="table" aria-label="대회 점수판"><div className="scoreboard-row header" role="row" style={{ gridTemplateColumns: columns }}><span>순위</span><span>참가자</span>{detail.problems.map((problem) => <span key={problem.label}>{problem.label}</span>)}<span>{board.scoring_mode === 'icpc' ? '해결 / 페널티' : '점수'}</span></div>{board.entries.map((entry) => <div className="scoreboard-row" role="row" style={{ gridTemplateColumns: columns }} key={entry.user_id}><strong>{entry.rank}</strong><a href={`/u/${entry.handle}`}><span>{entry.display_name}</span><small>@{entry.handle}</small></a>{detail.problems.map((problem) => { const cell = cellMap.get(`${entry.user_id}:${problem.label}`); return <span className={cell?.solved ? 'cell accepted' : cell?.frozen_attempts ? 'cell frozen' : 'cell'} key={problem.label}>{cell?.solved ? `+${Math.max(0, (cell.attempts ?? 1) - 1)}` : cell?.frozen_attempts ? `?${cell.frozen_attempts}` : cell?.attempts ? `-${cell.attempts}` : '·'}</span>; })}<b>{board.scoring_mode === 'icpc' ? `${entry.solved} / ${entry.penalty}` : entry.score}</b></div>)}</div>}</section></> : inviteGate ? <section className="contest-invite"><a className="back-link" href="/contests">← 대회 목록</a><p className="eyebrow">PRIVATE CONTEST</p><h1>초대받은 대회에 참가</h1><p>비공개 대회는 초대 코드를 입력하세요. 조직 대회 구성원은 코드 없이 확인할 수 있습니다.</p><label htmlFor="contest-code">초대 코드</label><input id="contest-code" value={joinCode} onChange={(event) => setJoinCode(event.target.value)} autoComplete="off" /><button type="button" onClick={join}>대회 참가</button></section> : !message && <p className="loading-state">대회를 불러오고 있습니다…</p>}{message && <p className="auth-message" role="alert">{message}</p>}</main></div>;
}

export default ContestDetailPage;
