#include <cstddef>
#include <new>
#include <utility>

#include "ryml.hpp"

static_assert(RYML_VERSION_MAJOR == 0 && RYML_VERSION_MINOR == 16 &&
                  RYML_VERSION_PATCH == 0,
              "yaml-rt-bench expects Rapid YAML v0.16.0");

extern "C" void *yaml_rt_rapidyaml_parse_in_arena(const char *data,
                                                    std::size_t size) noexcept
{
    try
    {
        ryml::Tree tree = ryml::parse_in_arena(ryml::csubstr(data, size));
        return new ryml::Tree(std::move(tree));
    }
    catch (...)
    {
        return nullptr;
    }
}

extern "C" void *yaml_rt_rapidyaml_parse_in_place(char *data,
                                                    std::size_t size) noexcept
{
    try
    {
        ryml::Tree tree = ryml::parse_in_place(ryml::substr(data, size));
        return new ryml::Tree(std::move(tree));
    }
    catch (...)
    {
        return nullptr;
    }
}

extern "C" void yaml_rt_rapidyaml_tree_delete(void *tree) noexcept
{
    delete static_cast<ryml::Tree *>(tree);
}
