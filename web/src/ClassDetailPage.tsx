import { FormEvent, useCallback, useEffect, useState } from "react";
import PortalHeader from "./PortalHeader";

type Assignment = {
  id: string;
  title: string;
  description: string;
  published_at: string;
  due_at: string;
  completion_goal_percent: number;
  total_items: number;
  completed_items: number;
};
type Item = {
  assignment_id: string;
  kind: string;
  slug: string;
  title: string;
  position: number;
  completed: boolean;
  completed_at: string | null;
};
type Detail = {
  class: {
    id: string;
    organization_name: string;
    name: string;
    viewer_role: string;
    learner_count: number;
  };
  assignments: Assignment[];
  items: Item[];
};
type Dashboard = {
  assignments: Array<{
    id: string;
    title: string;
    due_at: string;
    goal_percent: number;
    completion_percent: number;
    goal_met: boolean;
  }>;
  learners: Array<{
    user_id: string;
    handle: string;
    display_name: string;
    total_items: number;
    completed_items: number;
    completion_percent: number;
  }>;
  common_errors: Array<{ code: string; label: string; occurrences: number }>;
};
const csrfToken = () =>
  document.cookie
    .split(";")
    .map((cookie) => cookie.trim())
    .find((cookie) => cookie.startsWith("alpha_csrf="))
    ?.slice("alpha_csrf=".length) ?? "";
const dateTime = (value: string) =>
  new Intl.DateTimeFormat("ko-KR", {
    dateStyle: "medium",
    timeStyle: "short",
    timeZone: "Asia/Seoul",
  }).format(new Date(value));

function ClassDetailPage({ classId }: { classId: string }) {
  const [detail, setDetail] = useState<Detail | null>(null);
  const [dashboard, setDashboard] = useState<Dashboard | null>(null);
  const [message, setMessage] = useState("");
  const [invitePath, setInvitePath] = useState("");
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [dueAt, setDueAt] = useState("");
  const [itemSlugs, setItemSlugs] = useState("problem:alpha-pair-sum");
  const load = useCallback(async () => {
    const response = await fetch(`/api/v1/classes/${classId}`, {
      credentials: "include",
    });
    if (response.status === 401) {
      window.location.assign(
        `/login?redirect_after=${encodeURIComponent(`/classes/${classId}`)}`,
      );
      return;
    }
    const body = await response.json();
    if (!response.ok)
      throw new Error(body.error?.message ?? "학급을 불러오지 못했습니다.");
    setDetail(body);
    if (body.class.viewer_role === "INSTRUCTOR") {
      const dashboardResponse = await fetch(
        `/api/v1/classes/${classId}/instructor-dashboard`,
        { credentials: "include" },
      );
      if (dashboardResponse.ok) setDashboard(await dashboardResponse.json());
    }
  }, [classId]);
  useEffect(() => {
    load().catch((error) =>
      setMessage(
        error instanceof Error ? error.message : "학급을 불러오지 못했습니다.",
      ),
    );
  }, [load]);
  const invite = async () => {
    setMessage("");
    const response = await fetch(`/api/v1/classes/${classId}/invitations`, {
      method: "POST",
      credentials: "include",
      headers: {
        "content-type": "application/json",
        "x-csrf-token": csrfToken(),
      },
      body: JSON.stringify({ role: "learner", expires_in_days: 7 }),
    });
    const body = await response.json();
    if (!response.ok) {
      setMessage(body.error?.message ?? "초대를 만들지 못했습니다.");
      return;
    }
    setInvitePath(`${window.location.origin}${body.join_path}`);
  };
  const createAssignment = async (event: FormEvent) => {
    event.preventDefault();
    setMessage("");
    const items = itemSlugs
      .split(",")
      .map((value) => value.trim())
      .filter(Boolean)
      .map((value) => {
        const [kind, ...slug] = value.split(":");
        return { kind, slug: slug.join(":") };
      });
    const dueDate = new Date(dueAt);
    if (!dueAt || Number.isNaN(dueDate.getTime())) {
      setMessage("유효한 마감 시각을 입력해 주세요.");
      return;
    }
    try {
      const response = await fetch(`/api/v1/classes/${classId}/assignments`, {
        method: "POST",
        credentials: "include",
        headers: {
          "content-type": "application/json",
          "x-csrf-token": csrfToken(),
        },
        body: JSON.stringify({
          title,
          description,
          due_at: dueDate.toISOString(),
          completion_goal_percent: 80,
          items,
        }),
      });
      const body = await response.json();
      if (!response.ok) {
        setMessage(body.error?.message ?? "과제를 만들지 못했습니다.");
        return;
      }
      setTitle("");
      setDescription("");
      setDueAt("");
      await load();
    } catch {
      setMessage("과제를 만들지 못했습니다. 네트워크 상태를 확인해 주세요.");
    }
  };
  return (
    <div className="learning-page">
      <PortalHeader />
      <main className="class-detail-page">
        {detail ? (
          <>
            <a className="back-link" href="/classes">
              ← 내 교실
            </a>
            <section className="class-hero">
              <div>
                <p className="eyebrow">
                  {detail.class.viewer_role === "INSTRUCTOR"
                    ? "강사 학급"
                    : "내 학급"}
                </p>
                <h1>{detail.class.name}</h1>
                <p>
                  {detail.class.organization_name} · 학습자{" "}
                  {detail.class.learner_count}명
                </p>
              </div>
              {detail.class.viewer_role === "INSTRUCTOR" && (
                <button type="button" onClick={invite}>
                  학습자 초대
                </button>
              )}
            </section>
            {invitePath && (
              <label className="invite-result">
                <span>7일간 유효한 일회용 초대 링크</span>
                <input
                  value={invitePath}
                  readOnly
                  onFocus={(event) => event.currentTarget.select()}
                />
              </label>
            )}
            <section className="assignment-list">
              <h2>과제</h2>
              {detail.assignments.map((assignment) => (
                <article key={assignment.id}>
                  <header>
                    <div>
                      <span>
                        {new Date(assignment.due_at) < new Date()
                          ? "마감"
                          : `${dateTime(assignment.due_at)}까지`}
                      </span>
                      <h3>{assignment.title}</h3>
                      <p>{assignment.description}</p>
                    </div>
                    <strong>
                      {assignment.completed_items}/{assignment.total_items}
                    </strong>
                  </header>
                  <div>
                    {detail.items
                      .filter((item) => item.assignment_id === assignment.id)
                      .map((item) => (
                        <a
                          className={item.completed ? "completed" : ""}
                          href={
                            item.kind === "problem"
                              ? `/solve/${item.slug}`
                              : `/activities/${item.slug}`
                          }
                          key={`${item.kind}:${item.slug}`}
                        >
                          <span>
                            {item.position}. {item.title}
                          </span>
                          <b>
                            {item.completed
                              ? item.completed_at &&
                                new Date(item.completed_at) >
                                  new Date(assignment.due_at)
                                ? "마감 후 완료"
                                : "완료"
                              : item.kind === "problem"
                                ? "문제 풀기 →"
                                : "활동 시작 →"}
                          </b>
                        </a>
                      ))}
                  </div>
                </article>
              ))}
              {!detail.assignments.length && (
                <p className="dashboard-empty">현재 과제가 없습니다.</p>
              )}
            </section>
            {detail.class.viewer_role === "INSTRUCTOR" && (
              <>
                <section className="instructor-dashboard">
                  <div className="dashboard-heading">
                    <h2>수업 현황</h2>
                    <a href={`/api/v1/classes/${classId}/export.csv`}>
                      CSV 내보내기
                    </a>
                  </div>
                  <div className="class-goals">
                    {dashboard?.assignments.map((assignment) => (
                      <article
                        className={assignment.goal_met ? "met" : ""}
                        key={assignment.id}
                      >
                        <span>
                          {assignment.goal_met
                            ? "목표 달성"
                            : `목표 ${assignment.goal_percent}%`}
                        </span>
                        <strong>{assignment.completion_percent}%</strong>
                        <small>{assignment.title}</small>
                      </article>
                    ))}
                  </div>
                  <div className="instructor-columns">
                    <section>
                      <h3>학습자 진도</h3>
                      {dashboard?.learners.map((learner) => (
                        <div className="learner-progress" key={learner.user_id}>
                          <a href={`/u/${learner.handle}`}>
                            {learner.display_name}
                            <small>@{learner.handle}</small>
                          </a>
                          <progress
                            max="100"
                            value={learner.completion_percent}
                          />
                          <b>{learner.completion_percent}%</b>
                        </div>
                      ))}
                    </section>
                    <section>
                      <h3>공통 오류</h3>
                      {dashboard?.common_errors.map((error) => (
                        <div className="common-error" key={error.code}>
                          <span>{error.label}</span>
                          <b>{error.occurrences}회</b>
                        </div>
                      ))}
                      {!dashboard?.common_errors.length && (
                        <p>아직 집계된 오류가 없습니다.</p>
                      )}
                    </section>
                  </div>
                </section>
                <section className="assignment-create">
                  <div>
                    <p className="eyebrow">비공개 문제 세트</p>
                    <h2>새 과제 배정</h2>
                    <p>
                      항목은 쉼표로 구분해 <code>problem:슬러그</code> 또는{" "}
                      <code>activity:슬러그</code>로 입력합니다.
                    </p>
                  </div>
                  <form onSubmit={createAssignment}>
                    <label>
                      <span>과제 제목</span>
                      <input
                        value={title}
                        onChange={(event) => setTitle(event.target.value)}
                        required
                      />
                    </label>
                    <label>
                      <span>설명</span>
                      <textarea
                        value={description}
                        onChange={(event) => setDescription(event.target.value)}
                      />
                    </label>
                    <label>
                      <span>마감</span>
                      <input
                        type="datetime-local"
                        value={dueAt}
                        onChange={(event) => setDueAt(event.target.value)}
                        required
                      />
                    </label>
                    <label>
                      <span>학습 항목</span>
                      <input
                        value={itemSlugs}
                        onChange={(event) => setItemSlugs(event.target.value)}
                        required
                      />
                    </label>
                    <button type="submit">과제 공개</button>
                  </form>
                </section>
              </>
            )}
            {message && (
              <p className="auth-message" role="alert">
                {message}
              </p>
            )}
          </>
        ) : (
          !message && <p className="loading-state">학급을 불러오고 있습니다…</p>
        )}
        {message && !detail && (
          <p className="auth-message" role="alert">
            {message}
          </p>
        )}
      </main>
    </div>
  );
}

export default ClassDetailPage;
