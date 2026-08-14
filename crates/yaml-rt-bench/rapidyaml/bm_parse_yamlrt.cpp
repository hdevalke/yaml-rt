#include <cstddef>

extern "C" bool yaml_rt_bench_parse(const char *data, std::size_t length) noexcept;

// Include Rapid YAML's benchmark implementation so this adapter shares its
// private fixture and registers alongside every existing parser benchmark.
#include "../../../third_party/rapidyaml/bm/bm_parse.cpp"

void bm_ryml_yamlrt_arena(bm::State& st)
{
    c4::csubstr src = c4::to_csubstr(s_bm_case->src).trimr('\0');
    for(auto _ : st)
    {
        bool const parsed = yaml_rt_bench_parse(src.str, src.len);
        bm::DoNotOptimize(parsed);
        if(!parsed)
        {
            st.SkipWithError("yaml-rt rejected the benchmark fixture");
            break;
        }
    }
    s_bm_case->report(st);
}

BENCHMARK(bm_ryml_yamlrt_arena);
