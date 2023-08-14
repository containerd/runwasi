variable "CRATE" {
    default = null
}

# special target: https://github.com/docker/metadata-action#bake-definition
target "meta-helper" {}

group "default" {
    targets = ["image"]
}

target "image" {
    inherits = ["meta-helper"]
    output = ["type=image"]
    args = {
        "CRATE" = CRATE
    }
}

target "image-cross" {
    inherits = ["image"]
    platforms = ["linux/${arch}"]
    name = "image-cross-${image}-${arch}"
    matrix = {
        image = ["bullseye", "alpine"]
        arch = ["amd64", "arm64"]
    }
    args = {
        "BASE_IMAGE" = "${image}"
    }
}

target "bins" {
    inherits = ["image"]
    output= ["type=local,dest=bin/"]
}

target "bins-cross" {
    inherits = ["bins"]
    output= ["type=local,dest=bin/${image}-${arch}"]
    platforms = ["linux/${arch}"]
    name = "bins-cross-${image}-${arch}"
    matrix = {
        image = ["bullseye", "alpine"]
        arch = ["amd64", "arm64"]
    }
    args = {
        "BASE_IMAGE" = "${image}"
    }
}

target "tar" {
    inherits = ["bins"]
    output = ["type=local,dest=release/"]
    target = "release-tar"
}

target "tar-cross" {
    inherits = ["tar"]
    platforms = ["linux/${arch}"]
    name = "tar-cross-${image}-${arch}"
    matrix = {
        image = ["bullseye", "alpine"]
        arch = ["amd64", "arm64"]
    }
    args = {
        "BASE_IMAGE" = "${image}"
    }
}
