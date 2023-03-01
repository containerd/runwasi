def filter_by_package(package): if package != "" then map(select(.name == package)) else . end;

.packages | filter_by_package($CRATE) | map(.targets | map(select(.kind[] | contains("bin")).name))[] | select(length > 0)