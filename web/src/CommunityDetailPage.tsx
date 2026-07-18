import { FormEvent, useEffect, useState } from "react";
import PortalHeader from "./PortalHeader";

type Post = {
  id: string;
  kind: "notice" | "question" | "discussion";
  title: string;
  body: string;
  status: string;
  author_handle: string;
  author_name: string;
  problem_slug: string | null;
  problem_title: string | null;
  created_at: string;
};
type Answer = {
  id: string;
  body: string;
  author_handle: string;
  author_name: string;
  accepted: boolean;
  created_at: string;
};
type Detail = { post: Post; answers: Answer[]; viewer_is_author: boolean };
type ReportTarget = { type: "post" | "answer"; id: string };

const labels = { notice: "공지", question: "질문", discussion: "토론" };
const date = (value: string) =>
  new Intl.DateTimeFormat("ko-KR", {
    dateStyle: "medium",
    timeStyle: "short",
    timeZone: "Asia/Seoul",
  }).format(new Date(value));
const csrf = () =>
  document.cookie
    .split(";")
    .map((item) => item.trim())
    .find((item) => item.startsWith("alpha_csrf="))
    ?.slice("alpha_csrf=".length) ?? "";

function CommunityDetailPage({ postId }: { postId: string }) {
  const [detail, setDetail] = useState<Detail | null>(null);
  const [loggedIn, setLoggedIn] = useState(false);
  const [answer, setAnswer] = useState("");
  const [reportTarget, setReportTarget] = useState<ReportTarget | null>(null);
  const [reason, setReason] = useState("spam");
  const [reportDetail, setReportDetail] = useState("");
  const [message, setMessage] = useState("");

  const load = () =>
    fetch(`/api/v1/community/${postId}`, { credentials: "include" })
      .then(async (response) => {
        const payload = await response.json();
        if (!response.ok)
          throw new Error(payload.error?.message ?? "게시물을 찾지 못했습니다.");
        return payload;
      })
      .then(setDetail)
      .catch((error) =>
        setMessage(
          error instanceof Error ? error.message : "게시물을 찾지 못했습니다.",
        ),
      );
  useEffect(() => {
    load();
  }, [postId]);
  useEffect(() => {
    fetch("/api/v1/auth/me", { credentials: "include" })
      .then((response) => setLoggedIn(response.ok))
      .catch(() => setLoggedIn(false));
  }, []);

  const sendAnswer = async (event: FormEvent) => {
    event.preventDefault();
    const response = await fetch(`/api/v1/community/${postId}/answers`, {
      method: "POST",
      credentials: "include",
      headers: { "content-type": "application/json", "x-csrf-token": csrf() },
      body: JSON.stringify({ body: answer }),
    });
    if (response.status === 401) {
      window.location.assign(`/login?redirect_after=/community/${postId}`);
      return;
    }
    const payload = await response.json();
    if (!response.ok) {
      setMessage(payload.error?.message ?? "답변을 등록하지 못했습니다.");
      return;
    }
    setAnswer("");
    setMessage("답변을 등록했습니다.");
    load();
  };
  const accept = async (answerId: string) => {
    const response = await fetch(
      `/api/v1/community/${postId}/answers/${answerId}/accept`,
      {
        method: "POST",
        credentials: "include",
        headers: { "x-csrf-token": csrf() },
      },
    );
    if (!response.ok) {
      const payload = await response.json();
      setMessage(payload.error?.message ?? "답변을 채택하지 못했습니다.");
      return;
    }
    setMessage("답변을 채택했습니다.");
    load();
  };
  const report = async (event: FormEvent) => {
    event.preventDefault();
    if (!reportTarget) return;
    const response = await fetch("/api/v1/community/reports", {
      method: "POST",
      credentials: "include",
      headers: { "content-type": "application/json", "x-csrf-token": csrf() },
      body: JSON.stringify({
        target_type: reportTarget.type,
        target_id: reportTarget.id,
        reason,
        detail: reportDetail,
      }),
    });
    if (response.status === 401) {
      window.location.assign(`/login?redirect_after=/community/${postId}`);
      return;
    }
    if (!response.ok) {
      const payload = await response.json();
      setMessage(payload.error?.message ?? "신고를 접수하지 못했습니다.");
      return;
    }
    setReportTarget(null);
    setReportDetail("");
    setMessage("신고를 접수했습니다. 운영자가 확인합니다.");
  };

  return (
    <div className="learning-page">
      <PortalHeader />
      <main className="community-detail">
        <a className="community-back" href="/community">
          ← 커뮤니티로
        </a>
        {!detail && !message && <p className="loading-state">게시물을 불러오고 있습니다…</p>}
        {detail && (
          <>
            <article className="community-question">
              <div className="community-post-meta">
                <span className={`community-kind ${detail.post.kind}`}>
                  {labels[detail.post.kind]}
                </span>
                <span>{date(detail.post.created_at)}</span>
              </div>
              <h1>{detail.post.title}</h1>
              <div className="community-author">
                {detail.post.author_name} <span>@{detail.post.author_handle}</span>
              </div>
              {detail.post.problem_slug && (
                <a className="community-problem" href={`/problems/${detail.post.problem_slug}`}>
                  연결 문제 · {detail.post.problem_title}
                </a>
              )}
              <p className="community-body">{detail.post.body}</p>
              {detail.post.kind !== "notice" && (
                <button
                  className="text-action"
                  onClick={() =>
                    setReportTarget({ type: "post", id: detail.post.id })
                  }
                  type="button"
                >
                  게시물 신고
                </button>
              )}
            </article>

            <section className="community-answers">
              <h2>답변 {detail.answers.length}개</h2>
              {detail.answers.map((item) => (
                <article className={item.accepted ? "accepted" : ""} key={item.id}>
                  <header>
                    <div>
                      <strong>{item.author_name}</strong>
                      <span>
                        @{item.author_handle} · {date(item.created_at)}
                      </span>
                    </div>
                    {item.accepted && <b>채택된 답변</b>}
                  </header>
                  <p className="community-body">{item.body}</p>
                  <footer>
                    {detail.viewer_is_author &&
                      detail.post.kind === "question" &&
                      !item.accepted && (
                        <button onClick={() => accept(item.id)} type="button">
                          이 답변 채택
                        </button>
                      )}
                    <button
                      className="text-action"
                      onClick={() =>
                        setReportTarget({ type: "answer", id: item.id })
                      }
                      type="button"
                    >
                      신고
                    </button>
                  </footer>
                </article>
              ))}
              {!detail.answers.length && (
                <p className="dashboard-empty">아직 답변이 없습니다.</p>
              )}
            </section>

            {detail.post.kind !== "notice" && detail.post.status === "published" && (
              <section className="community-answer-form">
                <h2>근거를 담아 답변하기</h2>
                {loggedIn ? (
                  <form onSubmit={sendAnswer}>
                    <textarea
                      aria-label="답변 내용"
                      maxLength={10000}
                      value={answer}
                      onChange={(event) => setAnswer(event.target.value)}
                      required
                    />
                    <button type="submit">답변 등록</button>
                  </form>
                ) : (
                  <a href={`/login?redirect_after=/community/${postId}`}>
                    로그인하고 답변하기
                  </a>
                )}
              </section>
            )}
          </>
        )}

        {reportTarget && (
          <section className="community-report" aria-labelledby="report-title">
            <div>
              <h2 id="report-title">콘텐츠 신고</h2>
              <button onClick={() => setReportTarget(null)} type="button">
                닫기
              </button>
            </div>
            <form onSubmit={report}>
              <label>
                <span>사유</span>
                <select value={reason} onChange={(event) => setReason(event.target.value)}>
                  <option value="spam">도배·광고</option>
                  <option value="abuse">괴롭힘·혐오</option>
                  <option value="solution_leak">정답 유출</option>
                  <option value="privacy">개인정보 노출</option>
                  <option value="other">기타</option>
                </select>
              </label>
              <label>
                <span>운영자에게 알릴 내용 · 선택</span>
                <textarea
                  maxLength={2000}
                  value={reportDetail}
                  onChange={(event) => setReportDetail(event.target.value)}
                />
              </label>
              <button type="submit">신고 접수</button>
            </form>
          </section>
        )}
        {message && (
          <p className="auth-message" role="status">
            {message}
          </p>
        )}
      </main>
    </div>
  );
}

export default CommunityDetailPage;
