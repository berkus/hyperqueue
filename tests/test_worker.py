import time
from typing import Optional

from .conftest import HqEnv
from .utils import wait_until


def test_worker_list(hq_env: HqEnv):
    hq_env.start_server()
    table = hq_env.command(["worker", "list"], as_table=True)
    assert len(table) == 1
    assert table[0][:2] == ["Id", "State"]

    hq_env.start_workers(2)

    table = hq_env.command(["worker", "list"], as_table=True)
    assert len(table) == 3
    assert table[0][:2] == ["Id", "State"]
    assert table[1][:2] == ["1", "RUNNING"]
    assert table[2][:2] == ["2", "RUNNING"]

    hq_env.kill_worker(2)
    time.sleep(0.2)

    table = hq_env.command(["worker", "list"], as_table=True)
    assert len(table) == 3
    assert table[0][:2] == ["Id", "State"]
    assert table[1][:2] == ["1", "RUNNING"]
    assert table[2][:2] == ["2", "OFFLINE"]

    hq_env.kill_worker(1)
    time.sleep(0.2)

    table = hq_env.command(["worker", "list"], as_table=True)
    assert len(table) == 3
    assert table[0][:2] == ["Id", "State"]
    assert table[1][:2] == ["1", "OFFLINE"]
    assert table[2][:2] == ["2", "OFFLINE"]

    hq_env.start_worker()

    table = hq_env.command(["worker", "list"], as_table=True)

    assert len(table) == 4
    assert table[0][:2] == ["Id", "State"]
    assert table[1][:2] == ["1", "OFFLINE"]
    assert table[2][:2] == ["2", "OFFLINE"]
    assert table[3][:2] == ["3", "RUNNING"]


def test_worker_stop(hq_env: HqEnv):
    hq_env.start_server()
    process = hq_env.start_worker()

    worker_id = wait_until(lambda: get_worker_id(hq_env, 0))
    hq_env.command(["worker", "stop", worker_id])

    wait_until(
        lambda: hq_env.command(["worker", "list"], as_table=True)[1][1] == "OFFLINE"
    )
    hq_env.check_process_exited(process)


def get_worker_id(hq_env: HqEnv, index: int) -> Optional[str]:
    table = hq_env.command(["worker", "list"], as_table=True)
    if index >= len(table) - 1:
        return None
    return table[index + 1][0]


def test_worker_list_online_offline_state(hq_env: HqEnv):
    hq_env.start_server()
    hq_env.start_workers(2)
    table = hq_env.command(["worker", "list"], as_table=True)
    assert len(table) == 3
    assert table[1][:2] == ["1", "RUNNING"]
    assert table[2][:2] == ["2", "RUNNING"]
    hq_env.kill_worker(2)
    table = hq_env.command(["worker", "list"], as_table=True)
    assert len(table) == 3
    assert table[1][:2] == ["1", "RUNNING"]
    assert table[2][:2] == ["2", "OFFLINE"]

    table = hq_env.command(["worker", "list", "--offline"], as_table=True)
    assert len(table) == 2
    assert table[1][:2] == ["2", "OFFLINE"]

    table = hq_env.command(["worker", "list", "--running"], as_table=True)
    assert len(table) == 2
    assert table[1][:2] == ["1", "RUNNING"]


def test_worker_list_resources(hq_env: HqEnv):
    hq_env.start_server()
    hq_env.start_worker(cpus="10")
    hq_env.start_worker(cpus="4x5")

    table = hq_env.command(["worker", "list"], as_table=True)
    assert len(table) == 3
    assert table[1][:2] == ["1", "RUNNING"]
    assert table[2][:2] == ["2", "RUNNING"]
    assert table[0][3] == "Resources"
    assert table[1][3] == "1x10 cpus"
    assert table[2][3] == "4x5 cpus"