for f in 01_control_cli.txt 02_extract_pipeline.txt 03_symbols_pipeline.txt \
         04_chunking_gpt.txt 05_tree_and_walk.txt 06_infra_hotpaths.txt \
         07_config_and_docs.txt 08_tests_newline_index.txt
do
  rup chunk "$f" --model gpt-4o --max-tokens 4000 --overlap 128 --by-symbols -o "chunks/${f%.*}"
done
