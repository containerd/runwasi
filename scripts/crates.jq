def filter_by_package(package): if package != "" then map(select(.name == package)) else . end;

def get_bins: map(.targets | map(select(.kind[] | contains("bin")).name))[] | select(length > 0);
