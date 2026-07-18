import { FormEvent, useEffect, useState } from 'react';
import PortalHeader from './PortalHeader';

type ProblemTag = { tag: string; label: string };
type ProblemItem = {
  slug: string;
  title: string;
  difficulty: number | null;
  difficulty_source: 'alpha' | 'solved_ac';
  normalized_tier: string | null;
  learning_axis: string;
  source_kind: string;
  metadata_state: 'fresh' | 'stale' | 'blocked' | null;
  attribution: string | null;
  tags: ProblemTag[];
};

const axisLabels: Record<string, string> = {
  algorithmic_reasoning: '알고리즘 추론',
  code_literacy: '코드 읽기',
  docs_learning: '문서 기반 구현',
  independent_coding: '독립 코딩',
};

const tierLabel = (tier: string | null, level: number | null) => {
  if (level === null) return '난이도 확인 불가';
  if (tier === 'unrated' || level === 0) return '미평가 · 0';
  if (!tier) return `ALPHA ${level}`;
  const [group, division] = tier.split('-');
  const groups: Record<string, string> = { bronze: '브론즈', silver: '실버', gold: '골드', platinum: '플래티넘', diamond: '다이아몬드', ruby: '루비' };
  return `${groups[group] ?? '특수'} ${division?.toUpperCase() ?? level}`;
};

function ProblemsPage() {
  const [items, setItems] = useState<ProblemItem[]>([]);
  const [query, setQuery] = useState('');
  const [tag, setTag] = useState('');
  const [axis, setAxis] = useState('');
  const [minLevel, setMinLevel] = useState('');
  const [maxLevel, setMaxLevel] = useState('');
  const [message, setMessage] = useState('');
  const [loading, setLoading] = useState(true);

  const load = async (parameters = new URLSearchParams()) => {
    setLoading(true);
    setMessage('');
    try {
      const response = await fetch(`/api/v1/problems?${parameters}`);
      const body = await response.json();
      if (!response.ok) throw new Error(body.error?.message ?? '문제를 불러오지 못했습니다.');
      setItems(body.items);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : '문제를 불러오지 못했습니다.');
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { load(); }, []);

  const search = (event: FormEvent) => {
    event.preventDefault();
    const parameters = new URLSearchParams();
    if (query.trim()) parameters.set('q', query.trim());
    if (tag.trim()) parameters.set('tag', tag.trim());
    if (axis) parameters.set('axis', axis);
    if (minLevel) parameters.set('min_level', minLevel);
    if (maxLevel) parameters.set('max_level', maxLevel);
    load(parameters);
  };

  return (
    <div className="learning-page">
      <PortalHeader />
      <main className="catalog-panel" aria-labelledby="catalog-title">
        <p className="eyebrow">문제 탐색</p>
        <h1 id="catalog-title">근거를 찾는<br /><span>문제 탐색</span></h1>
        <p className="catalog-description">검색·태그·학습 축·난이도를 함께 좁혀 지금 필요한 문제를 찾습니다.</p>

        <form className="problem-filters" onSubmit={search}>
          <label className="search-field"><span>문제 검색</span><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="제목 또는 식별자" /></label>
          <label><span>태그</span><input value={tag} onChange={(event) => setTag(event.target.value)} placeholder="예: binary_search" pattern="[a-z0-9_]+" /></label>
          <label><span>학습 축</span><select value={axis} onChange={(event) => setAxis(event.target.value)}><option value="">전체</option>{Object.entries(axisLabels).map(([value, label]) => <option value={value} key={value}>{label}</option>)}</select></label>
          <label><span>최소 난이도</span><input type="number" min="0" max="30" value={minLevel} onChange={(event) => setMinLevel(event.target.value)} /></label>
          <label><span>최대 난이도</span><input type="number" min="0" max="30" value={maxLevel} onChange={(event) => setMaxLevel(event.target.value)} /></label>
          <button className="primary-action" type="submit">조건 적용</button>
        </form>

        {loading ? <p className="loading-state">문제 목록을 불러오고 있습니다…</p> : items.length ? (
          <div className="problem-list" aria-live="polite">
            {items.map((problem) => (
              <a className="problem-card" href={`/problems/${problem.slug}`} key={problem.slug}>
                <div>
                  <div className="problem-badges">
                    <span className={`tier-token ${problem.normalized_tier?.split('-')[0] ?? 'alpha'}`}>{tierLabel(problem.normalized_tier, problem.difficulty)}</span>
                    <span>{axisLabels[problem.learning_axis]}</span>
                    {problem.metadata_state === 'stale' && <span className="stale-token">마지막 정상 값</span>}
                    {problem.metadata_state === 'blocked' && <span className="blocked-token">연동 중단</span>}
                  </div>
                  <h2>{problem.title}</h2>
                  <div className="tag-row">{problem.tags.map((item) => <span key={item.tag}>#{item.label}</span>)}</div>
                </div>
                <div className="problem-source"><span>{problem.difficulty_source === 'solved_ac' ? 'solved.ac 메타데이터' : 'ALPHA 난이도'}</span>{problem.attribution && <small>{problem.attribution}</small>}</div>
              </a>
            ))}
          </div>
        ) : <section className="catalog-empty"><h2>조건에 맞는 문제가 없습니다</h2><p>검색어나 난이도 범위를 넓혀 보세요.</p></section>}
        {message && <p className="auth-message" role="alert">{message}</p>}
      </main>
    </div>
  );
}

export default ProblemsPage;
