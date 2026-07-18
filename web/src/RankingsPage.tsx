import { useEffect, useState } from 'react';
import PortalHeader from './PortalHeader';

type Entry = { rank: number; handle: string; display_name: string; score: number; tier: string | null; title: string | null };
const scopes = [
  ['', '전체 XP'], ['code_reading', '코드 독해'], ['debugging', '디버깅'], ['documentation', '문서 활용'],
  ['algorithm', '알고리즘'], ['contest', '대회'], ['season', '시작 시즌'],
];

function RankingsPage() {
  const initial = new URLSearchParams(window.location.search).has('season') ? 'season' : '';
  const [scope, setScope] = useState(initial);
  const [entries, setEntries] = useState<Entry[]>([]);
  const [message, setMessage] = useState('');

  useEffect(() => {
    const query = scope === 'season' ? '?season=launch-2026' : scope ? `?axis=${scope}` : '';
    fetch(`/api/v1/rankings${query}`).then(async (response) => {
      const body = await response.json();
      if (!response.ok) throw new Error(body.error?.message ?? '랭킹을 불러오지 못했습니다.');
      return body;
    }).then((body) => setEntries(body.entries)).catch((error) => setMessage(error instanceof Error ? error.message : '랭킹을 불러오지 못했습니다.'));
  }, [scope]);

  return <div className="learning-page"><PortalHeader /><main className="rankings-panel"><p className="eyebrow">공개 랭킹</p><div className="rankings-heading"><h1>서로 다른 성장을<br /><span>한 줄로 섞지 않아요</span></h1><label><span>랭킹 기준</span><select value={scope} onChange={(event) => setScope(event.target.value)}>{scopes.map(([value, label]) => <option value={value} key={label}>{label}</option>)}</select></label></div><p className="catalog-description">공개에 동의한 프로필만 표시하며 solved.ac 난이도와 ALPHA 사용자 역량은 별개입니다.</p><ol className="ranking-list">{entries.map((entry) => <li key={entry.handle}><strong>{entry.rank}</strong><a href={`/u/${entry.handle}`}><span>{entry.display_name}</span><small>@{entry.handle}{entry.title ? ` · ${entry.title}` : ''}</small></a><div><b>{entry.score.toLocaleString()}</b><small>{entry.tier ?? (scope ? '점' : 'XP')}</small></div></li>)}</ol>{!entries.length && !message && <p className="dashboard-empty">이 공개 범위에는 아직 기록이 없습니다.</p>}{message && <p className="auth-message" role="alert">{message}</p>}</main></div>;
}

export default RankingsPage;
