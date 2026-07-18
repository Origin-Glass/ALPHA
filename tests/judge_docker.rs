use alpha::{
    judge::{JudgeTestCase, LeasedJob, Verdict},
    sandbox::DockerSandbox,
};
use uuid::Uuid;

fn job(language: &str, source: &str, time_limit_ms: i32, memory_limit_mb: i32) -> LeasedJob {
    LeasedJob {
        job_id: Uuid::now_v7(),
        submission_id: Uuid::now_v7(),
        lease_token: Uuid::now_v7(),
        attempt: 1,
        language: language.to_owned(),
        source: source.to_owned(),
        problem_id: Uuid::now_v7(),
        problem_revision_id: Uuid::now_v7(),
        time_limit_ms,
        memory_limit_mb,
        checker_kind: "whitespace".to_owned(),
        float_tolerance: None,
        run_kind: "formal".to_owned(),
        custom_input: None,
    }
}

fn cases() -> Vec<JudgeTestCase> {
    vec![
        JudgeTestCase {
            ordinal: 1,
            input: "1 2\n".to_owned(),
            expected_output: "3\n".to_owned(),
            score_weight: 1,
            group_key: "main".to_owned(),
        },
        JudgeTestCase {
            ordinal: 2,
            input: "-5 7\n".to_owned(),
            expected_output: "2\n".to_owned(),
            score_weight: 1,
            group_key: "main".to_owned(),
        },
    ]
}

async fn verdict(sandbox: &DockerSandbox, job: LeasedJob) -> Verdict {
    sandbox.judge(&job, &cases()).await.unwrap().verdict
}

fn docs_capstone_cases() -> Vec<JudgeTestCase> {
    vec![
        JudgeTestCase {
            ordinal: 1,
            input: "HTTPS://Example.COM/path?b=2&a=1#section\n".to_owned(),
            expected_output: "https://example.com/path?a=1&b=2\n".to_owned(),
            score_weight: 1,
            group_key: "main".to_owned(),
        },
        JudgeTestCase {
            ordinal: 2,
            input: "https://EXAMPLE.com/p?z=&a=2&a=1#frag\n".to_owned(),
            expected_output: "https://example.com/p?a=1&a=2&z=\n".to_owned(),
            score_weight: 1,
            group_key: "main".to_owned(),
        },
        JudgeTestCase {
            ordinal: 3,
            input: "HTTP://Example.COM/%7Euser\n".to_owned(),
            expected_output: "http://example.com/%7Euser\n".to_owned(),
            score_weight: 1,
            group_key: "main".to_owned(),
        },
    ]
}

#[tokio::test]
#[ignore = "Docker 격리 런타임이 있는 CI에서 별도 실행"]
async fn three_languages_and_abuse_cases_receive_real_sandbox_verdicts() {
    let image =
        std::env::var("JUDGE_TEST_IMAGE").unwrap_or_else(|_| "alpha-judge-runner:test".to_owned());
    let sandbox = DockerSandbox::new("docker".to_owned(), image).unwrap();
    sandbox.verify().await.unwrap();

    assert_eq!(
        verdict(
            &sandbox,
            job(
                "cpp20",
                "#include <iostream>\nint main(){long long a,b;std::cin>>a>>b;std::cout<<a+b<<'\\n';}",
                1000,
                256,
            ),
        )
        .await,
        Verdict::Accepted
    );
    let mut custom = job(
        "python3",
        "values=list(map(int,input().split()))\nprint(sum(values))\n",
        1000,
        256,
    );
    custom.run_kind = "custom".to_owned();
    custom.custom_input = Some("4 5 6\n".to_owned());
    let custom_outcome = sandbox.judge(&custom, &cases()).await.unwrap();
    assert_eq!(custom_outcome.verdict, Verdict::Accepted);
    assert_eq!(custom_outcome.run_output.as_deref(), Some("15\n"));
    assert_eq!(
        verdict(
            &sandbox,
            job(
                "python3",
                "a,b=map(int,input().split())\nprint(a+b)\n",
                1000,
                256,
            ),
        )
        .await,
        Verdict::Accepted
    );
    assert_eq!(
        verdict(
            &sandbox,
            job(
                "java21",
                "import java.util.*; class Main { public static void main(String[] x){ Scanner s=new Scanner(System.in); System.out.println(s.nextLong()+s.nextLong()); }}",
                2000,
                512,
            ),
        )
        .await,
        Verdict::Accepted
    );
    assert_eq!(
        verdict(&sandbox, job("cpp20", "int main( {", 1000, 256)).await,
        Verdict::CompileError
    );
    assert_eq!(
        verdict(
            &sandbox,
            job("cpp20", "int main(){int* p=nullptr;*p=1;}", 1000, 256),
        )
        .await,
        Verdict::RuntimeError
    );
    assert_eq!(
        verdict(&sandbox, job("cpp20", "int main(){for(;;){}}", 300, 256)).await,
        Verdict::TimeLimitExceeded
    );
    assert_eq!(
        verdict(
            &sandbox,
            job(
                "cpp20",
                "#include <iostream>\nint main(){for(;;)std::cout<<\"xxxxxxxxxxxxxxxx\";}",
                2000,
                256,
            ),
        )
        .await,
        Verdict::OutputLimitExceeded
    );
    assert_eq!(
        verdict(
            &sandbox,
            job(
                "cpp20",
                "#include <vector>\nint main(){std::vector<char> x(1024ULL*1024*1024);for(auto& c:x)c=1;}",
                2000,
                64,
            ),
        )
        .await,
        Verdict::MemoryLimitExceeded
    );

    let blocked_capabilities = r#"
import os, socket
a,b=map(int,input().split())
safe=0
try:
    open('/etc/alpha-write-test','w').write('x')
except OSError:
    safe+=1
try:
    open('/hidden-tests/case-1')
except OSError:
    safe+=1
s=socket.socket(); s.settimeout(.2)
try:
    s.connect(('1.1.1.1',80))
except OSError:
    safe+=1
print(a+b if safe==3 else 'sandbox-bypass')
"#;
    assert_eq!(
        verdict(&sandbox, job("python3", blocked_capabilities, 1500, 256),).await,
        Verdict::Accepted
    );

    let fork_bomb = r#"
#include <unistd.h>
int main(){for(int i=0;i<10000;i++){if(fork()<0)return 0;}return 0;}
"#;
    let _contained_verdict = verdict(&sandbox, job("cpp20", fork_bomb, 800, 128)).await;
    assert_eq!(
        verdict(
            &sandbox,
            job(
                "python3",
                "a,b=map(int,input().split())\nprint(a+b)\n",
                1000,
                256,
            ),
        )
        .await,
        Verdict::Accepted,
        "프로세스 폭주 뒤에도 다음 격리 작업이 정상 실행돼야 한다"
    );

    let docs_source = r#"
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit
url = input().strip()
parts = urlsplit(url)
query = urlencode(sorted(parse_qsl(parts.query, keep_blank_values=True)))
print(urlunsplit((parts.scheme.lower(), parts.netloc.lower(), parts.path, query, '')))
"#;
    let mut docs_job = job("python3", docs_source, 2000, 128);
    docs_job.checker_kind = "exact".to_owned();
    assert_eq!(
        sandbox
            .judge(&docs_job, &docs_capstone_cases())
            .await
            .unwrap()
            .verdict,
        Verdict::Accepted,
        "공식 문서 트랙의 최종 프로그램이 실제 격리 판정 경계를 통과해야 한다"
    );
}
