# Benchmarks

This page presents benchmarks comparing prek vs pre-commit.

Caveats:

- Benchmark performance may vary based on hardware, OS, network conditions, and other factors.
- Benchmarks are not exhaustive; results may vary with different repositories and configurations.
- Prek is under active development; performance may improve over time.

Environment:

pre-commit version: 4.3.0
prek version: 0.2.0

OS: macOS 15.5
CPU: Apple M3 Pro
RAM: 18GB

## Cold installation

Here is a benchmark of installing hooks from [Apache Airflow](https://github.com/apache/airflow), which has a large and complex pre-commit configuration.

Steps:

```console
uv tool install prek@0.2.0
uv tool install pre-commit@4.3.0

git clone https://github.com/apache/airflow
cd airflow
git checkout 3.0.6

hyperfine \
    --prepare 'prek clean && pre-commit clean && uv cache clean' \
    --setup 'prek --version && pre-commit --version' \
    --runs 1 \
    'prek install-hooks' \
    'pre-commit install-hooks'
```

Results:

```
Benchmark 1: prek install-hooks
  Time (abs ≡):        18.395 s               [User: 11.234 s, System: 9.979 s]

Benchmark 2: pre-commit install-hooks
  Time (abs ≡):        186.990 s               [User: 68.774 s, System: 39.379 s]

Summary
  prek install-hooks ran
   10.17 times faster than pre-commit install-hooks
```

Disk usage after installation:

```console
$ du -sh ~/.cache/prek ~/.cache/pre-commit
810M	/Users/Jo/.cache/prek
1.6G	/Users/Jo/.cache/pre-commit
```

## Runtime benchmarks

Since some hooks might be slow to run (e.g., `cargo clippy`), which can take minutes, making any other overhead negligible, we choose to only run `check-toml` hook in `cpython` codebase.

### With prek fast path

Steps:

```console
git clone https://github.com/python/cpython
cd cpython
git checkout v3.14.0rc2

hyperfine \
    --warmup 3 \
    --setup 'prek --version && pre-commit --version' \
    --runs 5 \
    'prek run -a check-toml' \
    'pre-commit run -a check-toml'
```

Results:

```console
Benchmark 1: prek run -a check-toml
  Time (mean ± σ):      77.1 ms ±   2.5 ms    [User: 44.1 ms, System: 128.5 ms]
  Range (min … max):    75.1 ms …  81.3 ms    5 runs

Benchmark 2: pre-commit run -a check-toml
  Time (mean ± σ):     351.6 ms ±  25.0 ms    [User: 214.5 ms, System: 195.5 ms]
  Range (min … max):   332.8 ms … 393.2 ms    5 runs

Summary
  prek run -a check-toml ran
    4.56 ± 0.36 times faster than pre-commit run -a check-toml
```

### Without prek fast path

Steps:

```console
hyperfine \
    --warmup 3 \
    --setup 'prek --version && pre-commit --version' \
    --runs 5 \
    'PREK_NO_FAST_PATH=1 prek run -a check-toml' \
    'pre-commit run -a check-toml'
```

Results:

```
Benchmark 1: PREK_NO_FAST_PATH=1 prek run -a check-toml
  Time (mean ± σ):     137.3 ms ±   5.1 ms    [User: 111.0 ms, System: 147.5 ms]
  Range (min … max):   131.9 ms … 144.0 ms    5 runs

Benchmark 2: pre-commit run -a check-toml
  Time (mean ± σ):     397.6 ms ±  49.2 ms    [User: 217.6 ms, System: 197.7 ms]
  Range (min … max):   332.6 ms … 440.7 ms    5 runs

Summary
  PREK_NO_FAST_PATH=1 prek run -a check-toml ran
    2.90 ± 0.37 times faster than pre-commit run -a check-toml
```

## Benchmark from the community

- [Ready Prek Go!](https://hugovk.dev/blog/2025/ready-prek-go/) from Hugo van Kemenade.
