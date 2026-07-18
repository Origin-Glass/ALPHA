import { act,cleanup,fireEvent,render,screen,waitFor } from "@testing-library/react";
import { afterEach,expect,test,vi } from "vitest";
import ReviewWorkspacePage from "./ReviewWorkspacePage";

afterEach(()=>{cleanup();vi.useRealTimers();vi.unstubAllGlobals();});
const response=(payload:unknown)=>Promise.resolve({ok:true,status:200,json:async()=>payload});

test("검토 대상을 폴링하고 현재 단계에 맞는 사람 승인만 CSRF·멱등키로 보낸다",async()=>{
  vi.useFakeTimers();document.cookie="alpha_csrf=review-csrf; path=/";let loads=0;
  const fetchMock=vi.fn().mockImplementation((path:string,options?:RequestInit)=>{
    if(path==="/api/v1/content/reviews"){loads++;return response({reviews:[{id:"review-1",revision:1,state:loads>1?"human_review_pending":"ai_review_pending",artifact_hash:"abc",updated_at:"2026-07-19T00:00:00Z"}]});}
    if(path==="/api/v1/content/reviews/review-1"&&!options?.method)return response({id:"review-1",revision:1,state:"human_review_pending",artifact_hash:"abc",artifact:{title:"경계값 검토"},ai_receipts:[
      {kind:"specification_pedagogy",outcome:"pass",provider:"local",model:"m1",prompt_version:"spec-v1",latency_ms:100,usage:{}},
      {kind:"solution_judge",outcome:"pass",provider:"local",model:"m1",prompt_version:"judge-v1",latency_ms:110,usage:{}},
      {kind:"adversarial_rights",outcome:"pass",provider:"local",model:"m1",prompt_version:"rights-v1",latency_ms:120,usage:{}},
    ]});
    if(path==="/api/v1/content/reviews/review-1/human"&&options?.method==="POST")return response({state:"rights_review_pending"});
    throw new Error(`unexpected ${path}`);
  });vi.stubGlobal("fetch",fetchMock);vi.stubGlobal("crypto",{randomUUID:()=>"review-key"});
  const view=render(<ReviewWorkspacePage/>);await act(async()=>{await Promise.resolve();await Promise.resolve();});
  expect(screen.getByText(/외부 AI 검토 실행은 구성되지 않아 차단/)).toBeInTheDocument();
  expect(screen.queryByRole("button",{name:"사람 승인"})).not.toBeInTheDocument();
  await act(async()=>{await vi.advanceTimersByTimeAsync(3000);});fireEvent.click(screen.getByRole("button",{name:"검토 열기"}));await act(async()=>{await Promise.resolve();await Promise.resolve();});
  fireEvent.change(screen.getByLabelText("사람 검토 근거"),{target:{value:"세 검토 증거와 본문 일치 확인"}});fireEvent.click(screen.getByRole("button",{name:"사람 승인"}));
  await act(async()=>{await Promise.resolve();await Promise.resolve();});expect(screen.getByRole("status")).toHaveTextContent("권리 검토 대기");
  const call=fetchMock.mock.calls.find(([path,options])=>path.endsWith("/human")&&options?.method==="POST");expect(call?.[1]?.headers).toEqual({"content-type":"application/json","x-csrf-token":"review-csrf"});expect(JSON.parse(String(call?.[1]?.body)).idempotency_key).toBe("review-key");
  const calls=fetchMock.mock.calls.length;view.unmount();await act(async()=>{await vi.advanceTimersByTimeAsync(9000);});expect(fetchMock).toHaveBeenCalledTimes(calls);
});

test("응답 유실 재시도는 같은 게시 payload 멱등키를 유지한다",async()=>{
  document.cookie="alpha_csrf=review-csrf; path=/";let publishes=0;const keys:string[]=[];
  const detail={id:"review-2",revision:1,state:"approved",artifact_hash:"abc",artifact:{title:"승인 대상"},provenance:{id:"p1",provenance_hash:"hash"},ai_receipts:[],human_receipts:[],rights_receipts:[],pilot_receipts:[],audit:[]};
  const fetchMock=vi.fn().mockImplementation((path:string,options?:RequestInit)=>{
    if(path==="/api/v1/content/reviews")return response({reviews:[{id:"review-2",revision:1,state:"approved",artifact_hash:"abc",updated_at:"2026-07-19T00:00:00Z"}]});
    if(path==="/api/v1/content/reviews/review-2"&&!options?.method)return response(detail);
    if(path.endsWith("/publish")){keys.push(JSON.parse(String(options?.body)).idempotency_key);publishes++;if(publishes===1)return Promise.reject(new Error("응답 유실"));return response({state:"published"});}
    throw new Error(`unexpected ${path}`);
  });vi.stubGlobal("fetch",fetchMock);vi.stubGlobal("crypto",{randomUUID:()=>"stable-publish-key"});
  render(<ReviewWorkspacePage/>);await screen.findByText("게시 승인");fireEvent.click(screen.getByRole("button",{name:"검토 열기"}));await screen.findByRole("button",{name:"정확한 승인 리비전 게시"});
  fireEvent.click(screen.getByRole("button",{name:"정확한 승인 리비전 게시"}));await screen.findByText("응답 유실");fireEvent.click(screen.getByRole("button",{name:"정확한 승인 리비전 게시"}));
  await waitFor(()=>expect(keys).toEqual(["stable-publish-key","stable-publish-key"]));
});

test("제거 증거를 기록한 뒤 대체 리비전은 AI 검토부터 다시 시작한다",async()=>{
  document.cookie="alpha_csrf=review-csrf; path=/";let state="removal_pending";const writes:Array<{path:string;body:Record<string,unknown>}>=[];
  const detail=()=>({id:"review-3",revision:state==="ai_review_pending"?2:1,state,artifact_hash:"abc",artifact:{title:"제거 대상"},provenance:null,ai_receipts:[],human_receipts:[],rights_receipts:[],pilot_receipts:[],audit:[]});
  const fetchMock=vi.fn().mockImplementation((path:string,options?:RequestInit)=>{
    if(path==="/api/v1/content/reviews")return response({reviews:[{id:"review-3",revision:state==="ai_review_pending"?2:1,state,artifact_hash:"abc",updated_at:"2026-07-19T00:00:00Z"}]});
    if(path==="/api/v1/content/reviews/review-3"&&!options?.method)return response(detail());
    if(options?.method==="POST"){
      const body=JSON.parse(String(options.body));writes.push({path,body});
      if(path.endsWith("/removal/complete")){state="removal_completed";return response({state});}
      if(path.endsWith("/revise")){state="ai_review_pending";return response({state,revision:2});}
    }
    throw new Error(`unexpected ${path}`);
  });
  const keys=["removal-key","revision-key"];vi.stubGlobal("fetch",fetchMock);vi.stubGlobal("crypto",{randomUUID:()=>keys.shift()});
  render(<ReviewWorkspacePage/>);await screen.findByText("제거 대기");fireEvent.click(screen.getByRole("button",{name:"검토 열기"}));await screen.findByRole("button",{name:"제거 완료 기록"});
  expect(screen.queryByRole("button",{name:"AI 검토부터 다시 시작"})).not.toBeInTheDocument();
  fireEvent.change(screen.getByLabelText("제거 사유"),{target:{value:"배포본 제거 완료"}});fireEvent.change(screen.getByLabelText("제거 확인 증거"),{target:{value:"CDN과 공개 카탈로그 404 확인"}});fireEvent.click(screen.getByRole("button",{name:"제거 완료 기록"}));
  await screen.findByRole("button",{name:"AI 검토부터 다시 시작"});fireEvent.change(screen.getByLabelText("새 아티팩트 UUID"),{target:{value:"00000000-0000-7000-8000-000000000003"}});fireEvent.click(screen.getByRole("button",{name:"AI 검토부터 다시 시작"}));
  await waitFor(()=>expect(writes).toHaveLength(2));expect(writes[0]).toEqual({path:"/api/v1/content/reviews/review-3/removal/complete",body:{reason:"배포본 제거 완료",evidence:"CDN과 공개 카탈로그 404 확인",idempotency_key:"removal-key"}});expect(writes[1]).toEqual({path:"/api/v1/content/reviews/review-3/revise",body:{artifact_id:"00000000-0000-7000-8000-000000000003",idempotency_key:"revision-key"}});
  await screen.findByRole("heading",{name:"AI 검토 대기"});
});
