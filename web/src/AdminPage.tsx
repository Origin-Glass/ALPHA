import { FormEvent, MouseEvent, useEffect, useState } from "react";
import PortalHeader from "./PortalHeader";

type Viewer = { handle: string; roles: string[] };
type Report = {
  id: string;
  target_type: string;
  target_id: string;
  post_id: string;
  reason: string;
  detail: string;
  reporter_handle: string;
  target_preview: string;
  created_at: string;
};
type Worker = {
  worker_id: string;
  protocol_version: number;
  image_reference: string;
  status: string;
  health: string;
  current_job_id: string | null;
  last_heartbeat_at: string;
};
type Audit = {
  id: string;
  actor_handle: string | null;
  action: string;
  target_type: string;
  target_id: string | null;
  occurred_at: string;
};
type OperationalAction = { resource_id: string; state: string; reason: string; age_seconds: number };
type OperationalSnapshot = {
  providers: { unhealthy: number; unverified: number; disabled: number; action_items: OperationalAction[] };
  generation_jobs: { queued: number; running: number; expired_leases: number; failed: number; blocked: number; action_items: OperationalAction[] };
  reviews: { ai_pending: number; human_pending: number; rights_pending: number; pilot_pending: number; removal_pending: number; action_items: OperationalAction[] };
  rights: { publication_blockers: number; action_items: OperationalAction[] };
  learning: { active_projects: number; stalled_projects: number; action_items: OperationalAction[] };
  workspaces: { queued: number; running: number; expired_leases: number; failed: number; action_items: OperationalAction[] };
};

const csrf = () =>
  document.cookie
    .split(";")
    .map((item) => item.trim())
    .find((item) => item.startsWith("alpha_csrf="))
    ?.slice("alpha_csrf=".length) ?? "";
const date = (value: string) =>
  new Intl.DateTimeFormat("ko-KR", {
    dateStyle: "short",
    timeStyle: "short",
    timeZone: "Asia/Seoul",
  }).format(new Date(value));
const operationReason: Record<string, string> = {
  provider_disabled: "제공자 비활성화", provider_unhealthy: "제공자 응답 이상", provider_unverified: "제공자 검증 필요",
  expired_lease: "임대 만료", missing_credential: "자격 증명 없음", generation_failed: "생성 실패",
  ai_review_pending: "AI 검토 대기", human_review_pending: "사람 검토 대기", rights_review_pending: "권리 검토 대기",
  pilot_pending: "파일럿 검토 대기", removal_pending: "제거 대기", rights_not_approved: "권리 미승인",
  missing_provenance: "출처 근거 없음", missing_rights_approval: "권리 승인 없음", commercial_use_denied: "상업 이용 불허",
  redistribution_denied: "재배포 불허", stale_revision_evidence: "리비전 근거 만료", no_learning_event_7d: "7일간 학습 기록 없음", workspace_failed: "작업공간 실패",
};
const operationState: Record<string, string> = {
  disabled: "비활성", unhealthy: "응답 이상", unverified: "검증 필요", queued: "대기", leased: "실행 중",
  running: "실행 중", failed: "실패", blocked_disabled: "제공자 비활성으로 차단", blocked_missing_credential: "자격 증명 없어 차단",
  ai_review_pending: "AI 검토 대기", human_review_pending: "사람 검토 대기", rights_review_pending: "권리 검토 대기",
  pilot_pending: "파일럿 검토 대기", removal_pending: "제거 대기", approved: "승인", published: "게시 중",
  unpublished: "게시 해제", active: "진행 중",
};
const elapsed = (seconds: number) => seconds >= 3600
  ? `${Math.floor(seconds / 3600)}시간${seconds % 3600 >= 60 ? ` ${Math.floor(seconds % 3600 / 60)}분` : ""} 경과`
  : seconds >= 60 ? `${Math.floor(seconds / 60)}분 경과` : "1분 미만 경과";
const operationItemId = (domain: string, resourceId: string) => `operation-${domain}-${encodeURIComponent(resourceId)}`;
const operationContextHref = (domain: string, resourceId?: string) => resourceId
  ? `/admin?${new URLSearchParams({ operation_domain: domain, resource_id: resourceId })}#${operationItemId(domain, resourceId)}`
  : `/admin#operations-${domain}`;

function OperationSection({ id, title, metrics, actionLabel, actionHref, items, selectedResourceId, remediation, onAction, onSelectItem }: {
  id: string; title: string; metrics: Array<[string, number]>; actionLabel: string; actionHref: string; items: OperationalAction[]; selectedResourceId?: string; remediation?: string; onAction?: (event: MouseEvent<HTMLAnchorElement>) => void; onSelectItem?: (resourceId: string, event: MouseEvent<HTMLAnchorElement>) => void;
}) {
  return <section className="operation-section" aria-labelledby={`operations-${id}`}>
    <div className="operation-section-heading"><h3 id={`operations-${id}`}>{title}</h3><a href={actionHref} onClick={onAction}>{actionLabel}</a></div>
    <dl className="operation-metrics">{metrics.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}건</dd></div>)}</dl>
    {items.length ? <ul className="operation-items">{items.map((item, index) => {
      const selected = item.resource_id === selectedResourceId;
      return <li key={`${item.resource_id}-${item.reason}`} id={remediation ? operationItemId(id, item.resource_id) : undefined} className={selected ? "operation-item-selected" : undefined} tabIndex={selected ? -1 : undefined} aria-current={selected ? "true" : undefined} aria-label={`${title} 조치 ${index + 1}: ${operationReason[item.reason] ?? "알 수 없는 운영 사유"}, ${elapsed(item.age_seconds)}`}><strong>{operationReason[item.reason] ?? "알 수 없는 운영 사유"}</strong><span>{operationState[item.state] ?? "상태 확인 필요"} · {elapsed(item.age_seconds)}</span>{remediation && <><code>{item.resource_id}</code><p>{remediation}</p><a href={operationContextHref(id, item.resource_id)} onClick={(event) => onSelectItem?.(item.resource_id, event)}>이 항목 위치 열기</a></>}</li>;
    })}</ul> : <p className="dashboard-empty">조치할 운영 항목이 없습니다.</p>}
  </section>;
}

async function jsonRequest(url: string, options?: RequestInit) {
  const response = await fetch(url, { credentials: "include", ...options });
  const payload = response.status === 204 ? null : await response.json();
  if (!response.ok)
    throw new Error(payload?.error?.message ?? "요청을 처리하지 못했습니다.");
  return payload;
}

function AdminPage() {
  const operationQuery = new URLSearchParams(window.location.search);
  const [selectedOperation, setSelectedOperation] = useState(() => ({
    domain: operationQuery.get("operation_domain") ?? "",
    resource: operationQuery.get("resource_id") ?? "",
  }));
  const selectedOperationDomain = selectedOperation.domain;
  const selectedOperationResource = selectedOperation.resource;
  const [viewer, setViewer] = useState<Viewer | null>(null);
  const [reports, setReports] = useState<Report[]>([]);
  const [workers, setWorkers] = useState<Worker[]>([]);
  const [audits, setAudits] = useState<Audit[]>([]);
  const [operations, setOperations] = useState<OperationalSnapshot | null>(null);
  const [operationsLoading, setOperationsLoading] = useState(false);
  const [operationsError, setOperationsError] = useState("");
  const [message, setMessage] = useState("");
  const [resolutionNote, setResolutionNote] = useState("");

  const [problemMode, setProblemMode] = useState<"create" | "revise">("create");
  const [slug, setSlug] = useState("");
  const [title, setTitle] = useState("");
  const [statement, setStatement] = useState("");
  const [difficulty, setDifficulty] = useState(3);
  const [sampleInput, setSampleInput] = useState("");
  const [sampleOutput, setSampleOutput] = useState("");
  const [hiddenInput, setHiddenInput] = useState("");
  const [hiddenOutput, setHiddenOutput] = useState("");
  const [tag, setTag] = useState("implementation");
  const [tagLabel, setTagLabel] = useState("구현");
  const [rejudgeSlug, setRejudgeSlug] = useState("");
  const [rejudgeReason, setRejudgeReason] = useState("");

  const [noticeTitle, setNoticeTitle] = useState("");
  const [noticeBody, setNoticeBody] = useState("");
  const [noticePinned, setNoticePinned] = useState(false);
  const selectOperation = (domain: string, resource: string, event: MouseEvent<HTMLAnchorElement>) => {
    event.preventDefault();
    window.history.pushState({}, "", operationContextHref(domain, resource));
    setSelectedOperation({ domain, resource });
  };

  const loadRoleData = async (roles: string[]) => {
    const requests: Promise<void>[] = [];
    if (roles.some((role) => ["MODERATOR", "ADMIN"].includes(role))) {
      requests.push(jsonRequest("/api/v1/admin/community/reports").then((payload) => setReports(payload.items)));
    }
    if (roles.includes("ADMIN")) {
      requests.push(jsonRequest("/api/v1/admin/judge/workers").then((payload) => setWorkers(payload.items)));
      requests.push(jsonRequest("/api/v1/admin/audit?limit=30").then((payload) => setAudits(payload.items)));
    }
    const failed = (await Promise.allSettled(requests)).find((result) => result.status === "rejected");
    if (failed?.status === "rejected") {
      setMessage(failed.reason instanceof Error ? failed.reason.message : "운영 정보를 불러오지 못했습니다.");
    }
  };
  const loadOperations = async () => {
    setOperations(null);
    setOperationsLoading(true);
    setOperationsError("");
    try {
      setOperations(await jsonRequest("/api/v1/admin/operations"));
    } catch (error) {
      setOperationsError(error instanceof Error ? error.message : "운영 진단을 불러오지 못했습니다.");
    } finally {
      setOperationsLoading(false);
    }
  };
  useEffect(() => {
    fetch("/api/v1/auth/me", { credentials: "include" })
      .then(async (response) => {
        if (response.status === 401) {
          window.location.assign("/login?redirect_after=/admin");
          return null;
        }
        if (!response.ok) throw new Error("운영 권한을 확인하지 못했습니다.");
        return response.json();
      })
      .then((user: Viewer | null) => {
        if (!user) return;
        setViewer(user);
        void loadRoleData(user.roles);
        if (user.roles.includes("ADMIN")) void loadOperations();
      })
      .catch((error) => setMessage(error instanceof Error ? error.message : "운영 정보를 불러오지 못했습니다."));
  }, []);
  useEffect(() => {
    if (!operations || !selectedOperationResource || !["learning", "workspaces"].includes(selectedOperationDomain)) return;
    const items = selectedOperationDomain === "learning" ? operations.learning.action_items : operations.workspaces.action_items;
    if (items.some((item) => item.resource_id === selectedOperationResource)) {
      document.getElementById(operationItemId(selectedOperationDomain, selectedOperationResource))?.focus();
    }
  }, [operations, selectedOperationDomain, selectedOperationResource]);

  const canSetProblems = viewer?.roles.some((role) =>
    ["PROBLEM_SETTER", "ADMIN"].includes(role),
  );
  const canModerate = viewer?.roles.some((role) =>
    ["MODERATOR", "ADMIN"].includes(role),
  );
  const isAdmin = viewer?.roles.includes("ADMIN");
  const hasAnyRole = canSetProblems || canModerate;

  const saveProblem = async (event: FormEvent) => {
    event.preventDefault();
    setMessage("");
    const content = {
      title,
      statement,
      difficulty,
      learning_axis: "algorithmic_reasoning",
      status: "draft",
      time_limit_ms: 1000,
      memory_limit_mb: 128,
      checker_kind: "whitespace",
      float_tolerance: null,
      tags: [{ tag, label: tagLabel }],
      test_cases: [
        { input: sampleInput, expected_output: sampleOutput, visibility: "sample", score_weight: 1, group_key: "main" },
        { input: hiddenInput, expected_output: hiddenOutput, visibility: "hidden", score_weight: 1, group_key: "main" },
      ],
    };
    try {
      const payload = await jsonRequest(
        problemMode === "create"
          ? "/api/v1/admin/problems"
          : `/api/v1/admin/problems/${encodeURIComponent(slug)}/revisions`,
        {
          method: "POST",
          headers: { "content-type": "application/json", "x-csrf-token": csrf() },
          body: JSON.stringify(problemMode === "create" ? { slug, ...content } : content),
        },
      );
      setMessage(`문제 리비전 ${payload.version}을 저장했습니다.`);
      if (isAdmin) void loadOperations();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "문제를 저장하지 못했습니다.");
    }
  };
  const createNotice = async (event: FormEvent) => {
    event.preventDefault();
    try {
      await jsonRequest("/api/v1/admin/notices", {
        method: "POST",
        headers: { "content-type": "application/json", "x-csrf-token": csrf() },
        body: JSON.stringify({ title: noticeTitle, body: noticeBody, pinned: noticePinned }),
      });
      setNoticeTitle("");
      setNoticeBody("");
      setMessage("공지를 게시하고 감사 기록을 남겼습니다.");
      void loadRoleData(viewer?.roles ?? []);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "공지를 게시하지 못했습니다.");
    }
  };
  const rejudge = async (event: FormEvent) => {
    event.preventDefault();
    try {
      const payload = await jsonRequest(
        `/api/v1/admin/problems/${encodeURIComponent(rejudgeSlug)}/rejudge`,
        {
          method: "POST",
          headers: { "content-type": "application/json", "x-csrf-token": csrf() },
          body: JSON.stringify({ reason: rejudgeReason }),
        },
      );
      setMessage(`재채점 ${payload.submission_count}건을 큐에 등록하고 감사 기록을 남겼습니다.`);
      setRejudgeReason("");
      if (isAdmin) void loadOperations();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "재채점을 요청하지 못했습니다.");
    }
  };
  const resolve = async (reportId: string, action: "hide" | "dismiss") => {
    if (!resolutionNote.trim()) {
      setMessage("검토 근거를 먼저 입력해 주세요.");
      return;
    }
    try {
      await jsonRequest(`/api/v1/admin/community/reports/${reportId}/resolve`, {
        method: "POST",
        headers: { "content-type": "application/json", "x-csrf-token": csrf() },
        body: JSON.stringify({ action, note: resolutionNote }),
      });
      setResolutionNote("");
      setMessage(action === "hide" ? "콘텐츠를 숨기고 기록했습니다." : "신고를 기각하고 기록했습니다.");
      void loadRoleData(viewer?.roles ?? []);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "신고를 처리하지 못했습니다.");
    }
  };

  return (
    <div className="learning-page">
      <PortalHeader />
      <main className="admin-page">
        <div className="admin-hero">
          <div>
            <p className="eyebrow">ALPHA 운영</p>
            <h1>근거가 남는<br /><span>운영 콘솔</span></h1>
            <p>콘텐츠, 채점 워커, 감사 이벤트를 역할별 경계 안에서 관리합니다.</p>
          </div>
          <a href="/community">커뮤니티 보기</a>
        </div>

        {viewer && !hasAnyRole && (
          <section className="admin-forbidden">
            <h2>운영 권한이 없습니다</h2>
            <p>학습 기능은 계속 이용할 수 있습니다. 운영 역할이 필요하면 서비스 관리자에게 문의하세요.</p>
          </section>
        )}

        {canModerate && (
          <section className="admin-section">
            <div className="admin-section-heading">
              <div><p className="eyebrow">신고 검토 대기열</p><h2>열린 신고 {reports.length}건</h2></div>
              <label><span>처리 근거</span><input value={resolutionNote} onChange={(event) => setResolutionNote(event.target.value)} placeholder="정책 조항과 판단 근거" /></label>
            </div>
            <div className="report-queue">
              {reports.map((report) => (
                <article key={report.id}>
                  <header><strong>{report.reason}</strong><span>@{report.reporter_handle} · {date(report.created_at)}</span></header>
                  <p>{report.target_preview}</p>
                  {report.detail && <small>신고 내용 · {report.detail}</small>}
                  <div><a href={`/community/${report.post_id}`} target="_blank" rel="noreferrer">문맥 확인</a><button type="button" onClick={() => resolve(report.id, "hide")}>숨김 처리</button><button type="button" onClick={() => resolve(report.id, "dismiss")}>신고 기각</button></div>
                </article>
              ))}
              {!reports.length && <p className="dashboard-empty">검토할 신고가 없습니다.</p>}
            </div>
          </section>
        )}

        {canSetProblems && (
          <section className="admin-section problem-authoring">
            <div><p className="eyebrow">리비전 보존 출제</p><h2>원본 문제 출제</h2><p>새 리비전은 이전 제출의 테스트를 바꾸지 않습니다. 출제자는 자신이 만든 문제만 개정할 수 있습니다.</p><form className="rejudge-form" onSubmit={rejudge}><h3>감사 가능한 재채점</h3><label><span>문제 식별자</span><input value={rejudgeSlug} onChange={(event) => setRejudgeSlug(event.target.value.toLowerCase())} required /></label><label><span>재채점 사유 · 10자 이상</span><textarea minLength={10} maxLength={500} value={rejudgeReason} onChange={(event) => setRejudgeReason(event.target.value)} required /></label><button type="submit">재채점 요청</button></form></div>
            <form onSubmit={saveProblem}>
              <div className="admin-form-row">
                <label><span>작업</span><select value={problemMode} onChange={(event) => setProblemMode(event.target.value as "create" | "revise")}><option value="create">새 문제</option><option value="revise">기존 문제 개정</option></select></label>
                <label><span>문제 식별자</span><input pattern="[a-z0-9-]+" value={slug} onChange={(event) => setSlug(event.target.value.toLowerCase())} required /></label>
                <label><span>상태</span><input value="초안 · 게시 검토 필요" readOnly /></label>
                <label><span>난이도 · 0–30</span><input type="number" min="0" max="30" value={difficulty} onChange={(event) => setDifficulty(Number(event.target.value))} required /></label>
              </div>
              <label><span>한국어 제목</span><input maxLength={120} value={title} onChange={(event) => setTitle(event.target.value)} required /></label>
              <label><span>한국어 문제 설명</span><textarea maxLength={50000} value={statement} onChange={(event) => setStatement(event.target.value)} required /></label>
              <div className="admin-form-row two">
                <label><span>태그</span><input pattern="[a-z0-9_]+" value={tag} onChange={(event) => setTag(event.target.value.toLowerCase())} required /></label>
                <label><span>태그 이름</span><input value={tagLabel} onChange={(event) => setTagLabel(event.target.value)} required /></label>
              </div>
              <div className="test-case-grid">
                <fieldset><legend>예제 테스트 · 공개</legend><label><span>입력</span><textarea value={sampleInput} onChange={(event) => setSampleInput(event.target.value)} /></label><label><span>기대 출력</span><textarea value={sampleOutput} onChange={(event) => setSampleOutput(event.target.value)} /></label></fieldset>
                <fieldset><legend>정식 테스트 · 비공개</legend><label><span>입력</span><textarea value={hiddenInput} onChange={(event) => setHiddenInput(event.target.value)} /></label><label><span>기대 출력</span><textarea value={hiddenOutput} onChange={(event) => setHiddenOutput(event.target.value)} /></label></fieldset>
              </div>
              <button type="submit">리비전 저장</button>
            </form>
          </section>
        )}

        {isAdmin && (
          <>
            <section className="admin-section operations-panel" aria-labelledby="operations-title">
              <div className="admin-section-heading"><div><p className="eyebrow">운영 진단</p><h2 id="operations-title">운영 현황</h2><p>소유자 정보와 비밀값 없이 조치 상태를 표시하며, 전역 항목은 담당자에게 전달할 리소스 식별자만 제공합니다.</p></div><button type="button" onClick={() => void loadOperations()} disabled={operationsLoading}>운영 현황 다시 불러오기</button></div>
              {operationsLoading && <p role="status">운영 현황을 불러오는 중입니다.</p>}
              {operationsError && <p role="alert" className="admin-message">{operationsError}</p>}
              {operations && !operationsLoading && <div className="operations-grid">
                <OperationSection id="providers" title="AI 제공자" actionLabel="제공자 관리" actionHref="/provider-controls" metrics={[["응답 이상", operations.providers.unhealthy], ["검증 필요", operations.providers.unverified], ["비활성", operations.providers.disabled]]} items={operations.providers.action_items} />
                <OperationSection id="generation-jobs" title="생성 작업" actionLabel="생성 작업 확인" actionHref="/content-studio" metrics={[["대기", operations.generation_jobs.queued], ["실행", operations.generation_jobs.running], ["임대 만료", operations.generation_jobs.expired_leases], ["실패", operations.generation_jobs.failed], ["차단", operations.generation_jobs.blocked]]} items={operations.generation_jobs.action_items} />
                <OperationSection id="reviews" title="콘텐츠 검토" actionLabel="검토 대기열 확인" actionHref="/content-reviews" metrics={[["AI", operations.reviews.ai_pending], ["사람", operations.reviews.human_pending], ["권리", operations.reviews.rights_pending], ["파일럿", operations.reviews.pilot_pending], ["제거", operations.reviews.removal_pending]]} items={operations.reviews.action_items} />
                <OperationSection id="rights" title="게시 권리" actionLabel="게시 권리 확인" actionHref="/content-reviews" metrics={[["게시 차단", operations.rights.publication_blockers]]} items={operations.rights.action_items} />
                <OperationSection id="learning" title="학습 프로젝트" actionLabel="프로젝트 확인" actionHref={operationContextHref("learning", operations.learning.action_items[0]?.resource_id)} onAction={operations.learning.action_items[0] ? (event) => selectOperation("learning", operations.learning.action_items[0].resource_id, event) : undefined} onSelectItem={(resource, event) => selectOperation("learning", resource, event)} metrics={[["진행 중", operations.learning.active_projects], ["정체", operations.learning.stalled_projects]]} items={operations.learning.action_items} selectedResourceId={selectedOperationDomain === "learning" ? selectedOperationResource : undefined} remediation="학습 정체 원인을 확인하고 담당자에게 프로젝트 식별자를 전달하세요." />
                <OperationSection id="workspaces" title="작업공간" actionLabel="작업공간 확인" actionHref={operationContextHref("workspaces", operations.workspaces.action_items[0]?.resource_id)} onAction={operations.workspaces.action_items[0] ? (event) => selectOperation("workspaces", operations.workspaces.action_items[0].resource_id, event) : undefined} onSelectItem={(resource, event) => selectOperation("workspaces", resource, event)} metrics={[["대기", operations.workspaces.queued], ["실행", operations.workspaces.running], ["임대 만료", operations.workspaces.expired_leases], ["실패", operations.workspaces.failed]]} items={operations.workspaces.action_items} selectedResourceId={selectedOperationDomain === "workspaces" ? selectedOperationResource : undefined} remediation="실행 실패나 임대 만료 상태를 확인하고 담당자에게 실행 식별자를 전달하세요." />
              </div>}
            </section>
            <section className="admin-grid">
              <section className="admin-section notice-authoring">
                <p className="eyebrow">공지</p><h2>공지 게시</h2>
                <form onSubmit={createNotice}><label><span>제목</span><input value={noticeTitle} onChange={(event) => setNoticeTitle(event.target.value)} required /></label><label><span>내용</span><textarea value={noticeBody} onChange={(event) => setNoticeBody(event.target.value)} required /></label><label className="admin-check"><input type="checkbox" checked={noticePinned} onChange={(event) => setNoticePinned(event.target.checked)} /><span>상단 고정</span></label><button type="submit">공지 게시</button></form>
              </section>
              <section className="admin-section">
                <p className="eyebrow">채점 워커</p><h2>채점 워커</h2>
                <div className="worker-list">{workers.map((worker) => <article key={worker.worker_id}><div><strong>{worker.worker_id}</strong><span className={worker.health}>{worker.health === "healthy" ? "정상" : "응답 지연"}</span></div><small>{worker.status} · protocol {worker.protocol_version}</small><code>{worker.image_reference}</code><span>마지막 신호 {date(worker.last_heartbeat_at)}</span></article>)}{!workers.length && <p className="dashboard-empty">등록된 워커가 없습니다.</p>}</div>
              </section>
            </section>
            <section className="admin-section audit-section">
              <p className="eyebrow">감사 기록</p><h2>최근 감사 이벤트</h2>
              <div tabIndex={0} aria-label="최근 감사 이벤트 목록">{audits.map((audit) => <article key={audit.id}><span>{date(audit.occurred_at)}</span><strong>{audit.action}</strong><span>{audit.actor_handle ? `@${audit.actor_handle}` : "시스템"}</span><small>{audit.target_type} · {audit.target_id ?? "-"}</small></article>)}</div>
            </section>
          </>
        )}
        {message && <p className="admin-message" role="status">{message}</p>}
      </main>
    </div>
  );
}

export default AdminPage;
