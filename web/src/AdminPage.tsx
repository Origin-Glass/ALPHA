import { FormEvent, useEffect, useState } from "react";
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

async function jsonRequest(url: string, options?: RequestInit) {
  const response = await fetch(url, { credentials: "include", ...options });
  const payload = response.status === 204 ? null : await response.json();
  if (!response.ok)
    throw new Error(payload?.error?.message ?? "요청을 처리하지 못했습니다.");
  return payload;
}

function AdminPage() {
  const [viewer, setViewer] = useState<Viewer | null>(null);
  const [reports, setReports] = useState<Report[]>([]);
  const [workers, setWorkers] = useState<Worker[]>([]);
  const [audits, setAudits] = useState<Audit[]>([]);
  const [message, setMessage] = useState("");
  const [resolutionNote, setResolutionNote] = useState("");

  const [problemMode, setProblemMode] = useState<"create" | "revise">("create");
  const [slug, setSlug] = useState("");
  const [title, setTitle] = useState("");
  const [statement, setStatement] = useState("");
  const [difficulty, setDifficulty] = useState(3);
  const [status, setStatus] = useState<"draft" | "published">("draft");
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

  const loadOperations = async (roles: string[]) => {
    if (roles.some((role) => ["MODERATOR", "ADMIN"].includes(role))) {
      const reportPayload = await jsonRequest("/api/v1/admin/community/reports");
      setReports(reportPayload.items);
    }
    if (roles.includes("ADMIN")) {
      const [workerPayload, auditPayload] = await Promise.all([
        jsonRequest("/api/v1/admin/judge/workers"),
        jsonRequest("/api/v1/admin/audit?limit=30"),
      ]);
      setWorkers(workerPayload.items);
      setAudits(auditPayload.items);
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
        loadOperations(user.roles).catch((error) =>
          setMessage(error instanceof Error ? error.message : "운영 정보를 불러오지 못했습니다."),
        );
      })
      .catch((error) => setMessage(error instanceof Error ? error.message : "운영 정보를 불러오지 못했습니다."));
  }, []);

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
      status,
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
      if (isAdmin) loadOperations(viewer?.roles ?? []);
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
      loadOperations(viewer?.roles ?? []);
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
      if (isAdmin) loadOperations(viewer?.roles ?? []);
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
      loadOperations(viewer?.roles ?? []);
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
                <label><span>상태</span><select value={status} onChange={(event) => setStatus(event.target.value as "draft" | "published")}><option value="draft">초안</option><option value="published">공개</option></select></label>
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
              <div>{audits.map((audit) => <article key={audit.id}><span>{date(audit.occurred_at)}</span><strong>{audit.action}</strong><span>{audit.actor_handle ? `@${audit.actor_handle}` : "시스템"}</span><small>{audit.target_type} · {audit.target_id ?? "-"}</small></article>)}</div>
            </section>
          </>
        )}
        {message && <p className="admin-message" role="status">{message}</p>}
      </main>
    </div>
  );
}

export default AdminPage;
