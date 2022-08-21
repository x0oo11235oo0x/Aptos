import json
import multiprocessing
import os
import pwd
import re
import resource
import subprocess
import sys
import tempfile
import textwrap
import time
from contextlib import contextmanager
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from enum import Enum
from typing import Any, Callable, Generator, List, Optional, Sequence, Tuple, Union


@dataclass
class RunResult:
    exit_code: int
    output: bytes

    def unwrap(self) -> bytes:
        if self.exit_code != 0:
            raise Exception(self.output.decode("utf-8"))
        return self.output


class Shell:
    def run(self, command: Sequence[str], stream_output: bool = False) -> RunResult:
        raise NotImplementedError


@dataclass
class LocalShell(Shell):
    verbose: bool = False

    def run(self, command: Sequence[str], stream_output: bool = False) -> RunResult:
        # Write to a temp file, stream to stdout
        tmpname = tempfile.mkstemp()[1]
        with open(tmpname, 'wb') as writer, open(tmpname, 'rb') as reader:
            if self.verbose:
                print(f"+ {' '.join(command)}")
            process = subprocess.Popen(command, stdout=writer, stderr=writer)
            output = b""
            while process.poll() is None:
                chunk = reader.read()
                output += chunk
                if stream_output:
                    sys.stdout.write(chunk.decode("utf-8"))
                time.sleep(0.1)
            output += reader.read()
        return RunResult(process.returncode, output)


class FakeShell(Shell):
    def run(self, command: Sequence[str], stream_output: bool = False) -> RunResult:
        return RunResult(0, b'output')


def install_dependency(dependency: str) -> None:
    print(f"{dependency} is not currently installed")
    answer = os.getenv("FORGE_INSTALL_DEPENDENCIES")
    if not answer:
        answer = input("Would you like to install it now? (y/n) ").strip().lower()
    if answer in ("y", "yes", "yeet", "yessir", "si"):
        shell = LocalShell(True)
        shell.run(["pip3", "install", dependency], stream_output=True).unwrap()
    else:
        print(f"Please install click (pip install {dependency}) and try again")
        exit(1)


try:
    import click
except ImportError:
    install_dependency("click")
    import click

try:
    import psutil
except ImportError:
    install_dependency("psutil")
    import psutil


def get_current_user() -> str:
    return pwd.getpwuid(os.getuid())[0]


def get_utc_timestamp(dt: datetime) -> str:
    return dt.strftime("%Y-%m-%dT%H:%M:%S.000Z")


@click.group()
def main() -> None:
    # Check that the current directory is the root of the repository.
    if not os.path.exists('.git'):
        print('This script must be run from the root of the repository.')
        raise SystemExit(1)


def envoption(name: str, default: Optional[Any] = None) -> Any:
    return click.option(
        f"--{name.lower().replace('_', '-')}",
        default=lambda: os.getenv(name, default() if callable(default) else default),
        show_default=True,
    )


class Filesystem:
    def write(self, filename: str, contents: bytes) -> None:
        raise NotImplementedError()

    def read(self, filename: str) -> bytes:
        raise NotImplementedError()

    def mkstemp(self) -> str:
        raise NotImplementedError()


class FakeFilesystem(Filesystem):
    def write(self, filename: str, contents: bytes) -> None:
        print(f"Wrote {contents} to {filename}")

    def read(self, filename: str) -> bytes:
        return b"fake"

    def mkstemp(self) -> str:
        return "temp"


class LocalFilesystem(Filesystem):
    def write(self, filename: str, contents: bytes) -> None:
        with open(filename, 'wb') as f:
            f.write(contents)

    def read(self, filename: str) -> bytes:
        with open(filename, 'rb') as f:
            return f.read()

    def mkstemp(self) -> str:
        return tempfile.mkstemp()[1]

# o11y resources
INTERN_ES_DEFAULT_INDEX = "90037930-aafc-11ec-acce-2d961187411f"
INTERN_ES_BASE_URL = "https://es.intern.aptosdev.com"
INTERN_GRAFANA_BASE_URL = (
    "https://o11y.aptosdev.com/grafana/d/overview/overview?orgId=1&refresh=10s&"
    "var-Datasource=Remote%20Prometheus%20Intern"
)
DEVINFRA_ES_DEFAULT_INDEX = "d0bc5e20-badc-11ec-9a50-89b84ac337af"
DEVINFRA_ES_BASE_URL = "https://es.devinfra.aptosdev.com"
DEVINFRA_GRAFANA_BASE_URL = (
    "https://o11y.aptosdev.com/grafana/d/overview/overview?orgId=1&refresh=10s&"
    "var-Datasource=Remote%20Prometheus%20Devinfra"
)
HUMIO_LOGS_LINK = (
    "https://cloud.us.humio.com/k8s/search?query=%24forgeLogs%28validator_insta"
    "nce%3Dvalidator-0%29%20%7C%20$FORGE_NAMESPACE%20&live=true&start=24h&widge"
    "tType=list-view&columns=%5B%7B%22type%22%3A%22field%22%2C%22fieldName%22%3"
    "A%22%40timestamp%22%2C%22format%22%3A%22timestamp%22%2C%22width%22%3A180%7"
    "D%2C%7B%22type%22%3A%22field%22%2C%22fieldName%22%3A%22level%22%2C%22forma"
    "t%22%3A%22text%22%2C%22width%22%3A54%7D%2C%7B%22type%22%3A%22link%22%2C%22"
    "openInNewBrowserTab%22%3Atrue%2C%22style%22%3A%22button%22%2C%22hrefTempla"
    "te%22%3A%22https%3A%2F%2Fgithub.com%2Faptos-labs%2Faptos-core%2Fpull%2F%7B"
    "%7Bfields%5B%5C%22github_pr%5C%22%5D%7D%7D%22%2C%22textTemplate%22%3A%22%7"
    "B%7Bfields%5B%5C%22github_pr%5C%22%5D%7D%7D%22%2C%22header%22%3A%22Forge%2"
    "0PR%22%2C%22width%22%3A79%7D%2C%7B%22type%22%3A%22field%22%2C%22fieldName%"
    "22%3A%22k8s.namespace%22%2C%22format%22%3A%22text%22%2C%22width%22%3A104%7"
    "D%2C%7B%22type%22%3A%22field%22%2C%22fieldName%22%3A%22k8s.pod_name%22%2C%"
    "22format%22%3A%22text%22%2C%22width%22%3A126%7D%2C%7B%22type%22%3A%22field"
    "%22%2C%22fieldName%22%3A%22k8s.container_name%22%2C%22format%22%3A%22text%"
    "22%2C%22width%22%3A85%7D%2C%7B%22type%22%3A%22field%22%2C%22fieldName%22%3"
    "A%22message%22%2C%22format%22%3A%22text%22%7D%5D&newestAtBottom=true&showO"
    "nlyFirstLine=false"
)


def prometheus_port_forward() -> None:
    os.execvp("kubectl", ["kubectl", "port-forward", "prometheus", "9090"])


class Process:
    def name(self) -> str:
        raise NotImplementedError()
    
    def kill(self) -> None:
        raise NotImplementedError()


@dataclass
class FakeProcess(Process):
    _name: str

    def name(self) -> str:
        return self._name

    def kill(self) -> None:
        print("killing {self._name}")


class Processes:
    def processes(self) -> Generator[Process, None, None]:
        raise NotImplementedError()


@dataclass
class SystemProcess(Process):
    process: psutil.Process

    def name(self) -> str:
        return self.process.name()

    def kill(self) -> None:
        self.process.kill()


class SystemProcesses(Processes):
    def processes(self) -> Generator[Process, None, None]:
        for process in psutil.process_iter():
            yield SystemProcess(process)


class FakeProcesses(Processes):
    def processes(self) -> Generator[Process, None, None]:
        yield FakeProcess("concensus")


class ForgeState(Enum):
    RUNNING = "RUNNING"
    PASS = "PASS"
    FAIL = "FAIL"
    SKIP = "SKIP"
    EMPTY = "EMPTY"


class ForgeResult:
    state: ForgeState
    output: str
    start_time: datetime
    end_time: datetime

    @classmethod
    def from_args(cls, state: ForgeState, output: str) -> "ForgeResult":
        result = cls()
        result.state = state
        result.output = output
        return result

    @classmethod
    def empty(cls) -> "ForgeResult":
        return cls.from_args(ForgeState.EMPTY, "")

    @classmethod
    @contextmanager
    def with_context(cls, context: "ForgeContext") -> Generator["ForgeResult", None, None]:
        result = cls()
        result.state = ForgeState.RUNNING
        result.start_time = context.time.now()
        yield result
        result.end_time = context.time.now()
        if result.state not in (ForgeState.PASS, ForgeState.FAIL, ForgeState.SKIP):
            raise Exception("Forge result never entered terminal state")
        if result.output is None:
            raise Exception("Forge result didnt record output")

    def set_state(self, state: ForgeState) -> None:
        self.state = state

    def set_output(self, output: str) -> None:
        self.output = output

    def format(self) -> str:
        return f"Forge {self.state.value.lower()}ed"


class Time:
    def epoch(self) -> str:
        return self.now().strftime('%s')

    def now(self) -> datetime:
        raise NotImplementedError()


class SystemTime(Time):
    def now(self) -> datetime:
        return datetime.now(timezone.utc)


class FakeTime(Time):
    _now: datetime = datetime.fromisoformat("2022-07-29T00:00:00+00:00")

    def now(self) -> datetime:
        return self._now


@dataclass
class ForgeContext:
    shell: Shell
    filesystem: Filesystem
    processes: Processes
    time: Time

    # forge criteria
    forge_test_suite: str
    local_p99_latency_ms_threshold: str
    forge_runner_tps_threshold: str
    forge_runner_duration_secs: str
    
    # forge cluster options
    forge_namespace: str
    reuse_args: Sequence[str]
    keep_args: Sequence[str]
    haproxy_args: Sequence[str]

    # aws related options
    aws_account_num: str
    aws_region: str

    forge_image_tag: str
    forge_upgrade_image_tag: str
    forge_namespace: str
    forge_cluster_name: str

    github_actions: str

    def report(self, result: ForgeResult, outputs: List["ForgeFormatter"]) -> None:
        for formatter in outputs:
            self.filesystem.write(formatter.filename, formatter.format(self, result).encode())

    @property
    def forge_chain_name(self) -> str:
        forge_chain_name = self.forge_namespace.lstrip("aptos-")
        if "forge" not in forge_chain_name:
            forge_chain_name += "net"
        return forge_chain_name


@dataclass
class ForgeFormatter:
    filename: str
    _format: Callable[[ForgeContext, ForgeResult], str]

    def format(self, context: ForgeContext, result: ForgeResult) -> str:
        return self._format(context, result)


def format_report(context: ForgeContext, result: ForgeResult) -> str:
    report_lines = []
    recording = False
    for line in result.output.splitlines():
        if line in ("====json-report-begin===", "====json-report-end==="):
            recording = not recording
        elif recording:
            report_lines.append(line)
    if not report_lines:
        return "Forge test runner terminated"
    report_text = None
    try:
        report_text = json.loads("".join(report_lines)).get("text")
    except Exception as e:
        return "Forge report malformed: {}\n{}".format(e, '\n'.join(report_lines))
    if not report_text:
        return "Forge report text empty. See test runner output."
    else:
        return report_text


def get_validator_logs_link(
    forge_namespace: str,
    forge_chain_name: str,
    time_filter: Union[bool, Tuple[datetime, datetime]],
) -> str:
    es_base_url = DEVINFRA_ES_BASE_URL if "forge" in forge_chain_name else INTERN_ES_BASE_URL
    es_default_index = DEVINFRA_ES_DEFAULT_INDEX if "forge" in forge_chain_name else INTERN_ES_DEFAULT_INDEX
    val0_hostname = "aptos-node-0-validator-0"

    if time_filter is True:
        es_time_filter = "refreshInterval:(pause:!f,value:10000),time:(from:now-15m,to:now)"
    elif isinstance(time_filter, tuple):
        es_start_time = time_filter[0].strftime("%Y-%m-%dT%H:%M:%S.000Z")
        es_end_time = time_filter[1].strftime("%Y-%m-%dT%H:%M:%S.000Z")
        es_time_filter = f"refreshInterval:(pause:!t,value:0),time:(from:'{es_start_time}',to:'{es_end_time}')"
    else:
        raise Exception(f"Invalid refresh argument: {time_filter}")

    return f"""
        {es_base_url}/_dashboards/app/discover#/?
        _g=(filters:!(), {es_time_filter})
        &_a=(
            columns:!(_source),
            filters:!((
                '$state':(store:appState),
                meta:(
                    alias:!n,
                    disabled:!f,
                    index:'{es_default_index}',
                    key:chain_name,
                    negate:!f,
                    params:(query:{forge_chain_name}),
                    type:phrase
                ),
                query:(match_phrase:(chain_name:{forge_chain_name}))
            ),
            (
                '$state':(store:appState),
                meta:(
                    alias:!n,
                    disabled:!f,
                    index:'{es_default_index}',
                    key:namespace,
                    negate:!f,
                    params:(query:{forge_namespace}),
                    type:phrase
                ),
                query:(match_phrase:(namespace:{forge_namespace}))
            ),
            (
                '$state':(store:appState),
                meta:(
                    alias:!n,
                    disabled:!f,
                    index:'{es_default_index}',
                    key:hostname,
                    negate:!f,
                    params:(query:{val0_hostname}),
                    type:phrase),
                    query:(match_phrase:(hostname:{val0_hostname})
                )
            )),
            index:'{es_default_index}',
            interval:auto,query:(language:kuery,query:''),sort:!()
        )
    """.replace(" ", "").replace("\n", "")


def get_dashboard_link(
    forge_cluster_name: str,
    forge_namespace: str,
    forge_chain_name: str,
    time_filter: Union[bool, Tuple[datetime, datetime]]
) -> str:
    if time_filter is True:
        grafana_time_filter = "&refresh=10s&from=now-15m&to=now"
    elif isinstance(time_filter, tuple):
        milliseconds = lambda dt: int(dt.strftime("%f")) / 1000
        start_ms = milliseconds(time_filter[0])
        end_ms = milliseconds(time_filter[1])
        grafana_time_filter = f"&from={start_ms}&to={end_ms}"
    else:
        raise Exception(f"Invalid refresh argument: {time_filter}")

    base_url = DEVINFRA_GRAFANA_BASE_URL if "forge" in forge_cluster_name else INTERN_GRAFANA_BASE_URL
    return f"{base_url}&var-namespace={forge_namespace}&var-chain_name={forge_chain_name}{grafana_time_filter}"



def get_humio_logs_link(forge_namespace: str) -> str:
    return HUMIO_LOGS_LINK.replace("$FORGE_NAMESPACE", forge_namespace)


def format_pre_comment(context: ForgeContext) -> str:
    dashboard_link = "https://banana"
    validator_logs_link = get_validator_logs_link(context.forge_namespace, context.forge_chain_name, True)
    humio_logs_link = get_humio_logs_link(context.forge_namespace)

    return textwrap.dedent(
        f"""
        =====START PRE_FORGE COMMENT=====
        ### Forge is running with `{context.forge_image_tag}`
        * [Grafana dashboard (auto-refresh)]({dashboard_link})
        * [Validator 0 logs (auto-refresh)]({validator_logs_link})
        * [Humio Logs]({humio_logs_link})
        =====END PRE_FORGE COMMENT=====
        """
    )


def format_comment(context: ForgeContext, result: ForgeResult) -> str:
    dashboard_link = get_dashboard_link(
        context.forge_cluster_name,
        context.forge_namespace,
        context.forge_chain_name,
        (result.start_time, result.end_time),
    )
    validator_logs_link = get_validator_logs_link(
        context.forge_namespace,
        context.forge_chain_name,
        (result.start_time, result.end_time),
    )
    humio_logs_link = get_humio_logs_link(context.forge_namespace)


    return textwrap.dedent(
        f"""
        =====START FORGE COMMENT=====
        ```
        ```
        ### Forge is running with `{context.forge_image_tag}`
        * [Grafana dashboard (auto-refresh)]({dashboard_link})
        * [Validator 0 logs (auto-refresh)]({validator_logs_link})
        * [Humio Logs]({humio_logs_link})
        {result.format()}
        =====END FORGE COMMENT=====
        """
    )


class ForgeRunner:
    def run(self, context: ForgeContext) -> ForgeResult:
        raise NotImplementedError


class LocalForgeRunner(ForgeRunner):
    def run(self, context: ForgeContext) -> ForgeResult:
        # Set rlimit to unlimited
        resource.setrlimit(resource.RLIMIT_NOFILE, (resource.RLIM_INFINITY, resource.RLIM_INFINITY))
        # Using fork can crash the subprocess, use spawn instead
        multiprocessing.set_start_method('spawn')
        port_forward_process = multiprocessing.Process(daemon=True, target=prometheus_port_forward)
        port_forward_process.start()
        with ForgeResult.with_context(context) as forge_result:
            result = context.shell.run([
                "cargo", "run", "-p", "forge-cli",
                "--",
                "--suite", context.forge_test_suite,
                "--mempool-backlog", "5000",
                "--avg-tps", context.forge_runner_tps_threshold,
                "--max-latency-ms", context.local_p99_latency_ms_threshold,
                "--duration-secs", context.forge_runner_duration_secs,
                "test", "k8s-swarm",
                "--image-tag", context.forge_image_tag,
                "--upgrade-image-tag", context.forge_upgrade_image_tag,
                "--namespace", context.forge_namespace,
                "--port-forward",
                *context.reuse_args,
                *context.keep_args,
                *context.haproxy_args,
            ], stream_output=True)
            try:
                forge_result.set_output(result.unwrap().decode())
                forge_result.set_state(ForgeState.PASS)
            except Exception as e:
                forge_result.set_output(str(e))
                forge_result.set_state(ForgeState.FAIL)

        # Kill port forward unless we're keeping them
        if not context.keep_args:
            # Kill all processess with kubectl in the name
            for process in psutil.process_iter():
                if 'kubectl' in process.name():
                    process.kill()
            port_forward_process.terminate()
            port_forward_process.join()

        return forge_result



class K8sForgeRunner(ForgeRunner):
    def run(self, context: ForgeContext) -> ForgeResult:
        forge_pod_name = f"{context.forge_namespace}-{context.time.epoch()}-{context.forge_image_tag}"[:64]
        context.shell.run([
            "kubectl", "delete", "pod",
            "-n", "default",
            "-l", f"forge-namespace={context.forge_namespace}",
            "--force"
        ])
        context.shell.run([
            "kubectl", "wait",
            "-n", "default",
            "--for=delete", "pod",
            "-l", f"forge-namespace={context.forge_namespace}",
        ])
        template = context.filesystem.read("testsuite/forge-test-runner-template.yaml")
        forge_triggered_by = "github-actions" if context.github_actions else "other"
        rendered = template.decode().format(
            FORGE_POD_NAME=forge_pod_name,
            FORGE_TEST_SUITE=context.forge_test_suite,
            FORGE_RUNNER_DURATION_SECS=context.forge_runner_duration_secs,
            FORGE_RUNNER_TPS_THRESHOLD=context.forge_runner_tps_threshold,
            IMAGE_TAG=context.forge_image_tag,
            AWS_ACCOUNT_NUM=context.aws_account_num,
            AWS_REGION=context.aws_region,
            FORGE_NAMESPACE=context.forge_namespace,
            REUSE_ARGS=context.reuse_args if context.reuse_args else "",
            KEEP_ARGS=context.keep_args if context.keep_args else "",
            ENABLE_HAPROXY_ARGS=context.haproxy_args if context.haproxy_args else "",
            FORGE_TRIGGERED_BY=forge_triggered_by,
            UPGRADE_IMAGE_TAG=context.forge_upgrade_image_tag,
        )

        with ForgeResult.with_context(context) as forge_result:
            specfile = context.filesystem.mkstemp()
            context.filesystem.write(specfile, rendered.encode())
            context.shell.run([
                "kubectl", "apply", "-n", "default", "-f", specfile
            ])
            context.shell.run([
                "kubectl", "wait", "-n", "default", "--timeout=5m", "--for=condition=Ready", f"pod/{forge_pod_name}"
            ])
            forge_logs = context.shell.run([
                "kubectl", "logs", "-n", "default", "-f", forge_pod_name
            ], stream_output=True).unwrap()

            state = None
            attempts = 100
            while state is None:
                # parse the pod status: https://kubernetes.io/docs/concepts/workloads/pods/pod-lifecycle/#pod-phase
                forge_status = context.shell.run([
                    "kubectl", "get", "pod", "-n", "default", forge_pod_name, "-o", "jsonpath='{.status.phase}'"
                ]).unwrap().decode().lower()

                if "running" in forge_status:
                    continue
                elif "succeeded" in forge_status:
                    state = ForgeState.PASS
                elif re.findall(r"not\s*found", forge_status, re.IGNORECASE):
                    state = ForgeState.SKIP
                else:
                    state = ForgeState.FAIL

                attempts -= 1
                if attempts <= 0:
                    raise Exception("Exhausted attempt to get forge pod status")

            forge_result.set_output(forge_logs.decode())
            forge_result.set_state(state)

        return forge_result


class AwsError(Exception):
    pass


def assert_aws_token_expiration(aws_token_expiration: Optional[str]) -> None:
    if aws_token_expiration is None:
        raise AwsError("AWS token is required")
    try:
        expiration = datetime.strptime(aws_token_expiration, "%Y-%m-%dT%H:%M:%S%z")
    except Exception as e:
        raise AwsError(f"Invalid date format: {aws_token_expiration}")
    if datetime.now(timezone.utc) > expiration:
        raise AwsError("AWS token has expired")


def get_aws_account_num(shell: Shell) -> str:
    caller_id = shell.run(["aws", "sts", "get-caller-identity"])
    return json.loads(caller_id.unwrap()).get("Account")


# TODO this is a bit gross to test properly
def update_aws_auth(shell: Shell, aws_auth_script: Optional[str] = None) -> str:
    if aws_auth_script is None:
        raise AwsError("Please authenticate with AWS and rerun")
    result = shell.run(["bash", "-c", f"source {aws_auth_script} && env | grep AWS_"])
    for line in result.unwrap().decode().splitlines():
        if line.startswith("AWS_"):
            key, val = line.split("=", 1)
            os.environ[key] = val
    return get_aws_account_num(shell)


def get_current_cluster_name(shell: Shell) -> str:
    result = shell.run(["kubectl", "config", "current-context"])
    current_context = result.unwrap().decode()
    matches = re.findall(r"aptos.*", current_context)
    if len(matches) != 1:
        raise ValueError("Could not determine current cluster name: {current_context}")
    return matches[0]


@dataclass
class Git:
    shell: Shell

    def run(self, command) -> RunResult:
        return self.shell.run(["git", *command])

    def last(self, limit: int = 1) -> Generator[str, None, None]:
        for i in range(limit):
            yield self.run(["rev-parse", f"HEAD~{i}"]).unwrap().decode()


def find_recent_image(shell: Shell, git: Git, commit_threshold: int = 100) -> str:
    # With stacks its very possible the last 5 commits arent built
    for revision in git.last(commit_threshold):
        if image_exists(shell, revision):
            return revision
    raise Exception("Couldnt find a recent built image")


def image_exists(shell: Shell, image_tag: str) -> bool:
    result = shell.run([
        "aws", "ecr", "describe-images",
        "--repository-name", "aptos/validator",
        "--image-ids", f"imageTag={image_tag}"
    ])
    return result.exit_code == 0


@main.command()
# for calculating regression in local mode
@envoption("LOCAL_P99_LATENCY_MS_THRESHOLD", "60000")
# output files
@envoption("FORGE_OUTPUT")
@envoption("FORGE_REPORT")
@envoption("FORGE_PRE_COMMENT")
@envoption("FORGE_COMMENT")
# cluster auth
@envoption("AWS_REGION", "us-west-2")
@envoption("AWS_TOKEN_EXPIRATION")
@envoption("AWS_AUTH_SCRIPT")
# forge test runner customization
@envoption("FORGE_RUNNER_MODE", "k8s")
@envoption("FORGE_CLUSTER_NAME")
@envoption("FORGE_NAMESPACE_KEEP")
@envoption("FORGE_NAMESPACE_REUSE")
@envoption("FORGE_ENABLE_HAPROXY")
@envoption("FORGE_TEST_SUITE", "land_blocking")
@envoption("FORGE_RUNNER_DURATION_SECS", "300")
@envoption("FORGE_RUNNER_TPS_THRESHOLD", "400")
@envoption("IMAGE_TAG")
@envoption("UPGRADE_IMAGE_TAG")
@envoption("FORGE_NAMESPACE")
@envoption("VERBOSE")
@envoption("GITHUB_ACTIONS", "false")
@click.option("--dry-run", is_flag=True)
@click.option("--ignore-cluster-warning", is_flag=True)
def test(
    local_p99_latency_ms_threshold: str,
    forge_output: Optional[str],
    forge_report: Optional[str],
    forge_pre_comment: Optional[str],
    forge_comment: Optional[str],
    aws_region: str,
    aws_token_expiration: Optional[str],
    aws_auth_script: Optional[str],
    forge_runner_mode: str,
    forge_cluster_name: Optional[str],
    forge_namespace_keep: Optional[str],
    forge_namespace_reuse: Optional[str],
    forge_enable_haproxy: Optional[str],
    forge_test_suite: str,
    forge_runner_duration_secs: str,
    forge_runner_tps_threshold: str,
    image_tag: Optional[str],
    upgrade_image_tag: Optional[str],
    forge_namespace: Optional[str],
    verbose: Optional[str],
    github_actions: str,
    dry_run: Optional[bool],
    ignore_cluster_warning: Optional[bool],
) -> None:
    shell = FakeShell() if dry_run else LocalShell(verbose == "true")
    git = Git(shell)
    filesystem = LocalFilesystem()
    processes = FakeProcesses() if dry_run else SystemProcesses()
    time = FakeTime() if dry_run else SystemTime()

    if dry_run:
        aws_account_num = "1234"
    # Pre flight checks
    else:
        try:
            aws_account_num = get_aws_account_num(shell)
        except Exception:
            aws_account_num = update_aws_auth(shell, aws_auth_script)

    if aws_auth_script:
        assert_aws_token_expiration(os.getenv("AWS_TOKEN_EXPIRATION"))

    if forge_cluster_name is None:
        forge_cluster_name = get_current_cluster_name(shell)

    if forge_namespace is None:
        forge_namespace = f"forge-{get_current_user()}-{time.epoch()}"

    assert aws_account_num is not None, "AWS account number is required"
    assert forge_namespace is not None, "Forge namespace is required"
    assert forge_cluster_name is not None, "Forge cluster name is required"

    click.echo(f"Using forge cluster: {forge_cluster_name}")
    if "forge" not in forge_cluster_name and not ignore_cluster_warning:
        click.echo("Forge cluster usually contains forge, to ignore this warning set --ignore-cluster-warning")
        return

    if image_tag is None:
        image_tag = find_recent_image(shell, git)
    else:
        if not image_exists(shell, image_tag):
            raise Exception(f"Image {image_tag} does not exist")
        if upgrade_image_tag and not image_exists(shell, upgrade_image_tag):
            raise Exception(f"Upgrade image {upgrade_image_tag} does not exist")

    assert image_tag is not None, "Image tag must be set"

    context = ForgeContext(
        shell=shell,
        filesystem=filesystem,
        processes=processes,
        time=time,

        forge_test_suite=forge_test_suite,
        local_p99_latency_ms_threshold=local_p99_latency_ms_threshold,
        forge_runner_tps_threshold=forge_runner_tps_threshold,
        forge_runner_duration_secs=forge_runner_duration_secs,

        reuse_args=["--reuse"] if forge_namespace_reuse else [],
        keep_args=["--keep"] if forge_namespace_keep else [],
        haproxy_args=["--enable-haproxy"] if forge_enable_haproxy else [],

        aws_account_num=aws_account_num,
        aws_region=aws_region,

        forge_image_tag=image_tag,
        forge_upgrade_image_tag=upgrade_image_tag or image_tag,
        forge_namespace=forge_namespace,
        forge_cluster_name=forge_cluster_name,

        github_actions=github_actions,
    )
    forge_runner_mapping = {
        'local': LocalForgeRunner,
        'k8s': K8sForgeRunner,
    }

    # Maybe this should be its own command?
    pre_comment = format_pre_comment(context)
    if forge_pre_comment:
        context.report(
            ForgeResult.empty(),
            [ForgeFormatter(forge_pre_comment, lambda *_: pre_comment)],
        )
    if forge_runner_mode == 'pre-forge':
        return

    forge_runner = forge_runner_mapping[forge_runner_mode]()
    result = forge_runner.run(context)

    print(result.format())

    outputs = []
    if forge_output:
        outputs.append(ForgeFormatter(forge_output, lambda *_: result.output))
    if forge_report:
        outputs.append(ForgeFormatter(forge_report, format_report))
    if forge_comment:
        outputs.append(ForgeFormatter(forge_comment, format_comment))
    context.report(result, outputs)

if __name__ == "__main__":
    main()