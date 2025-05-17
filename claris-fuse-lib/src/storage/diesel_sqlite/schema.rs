// @generated automatically by Diesel CLI.

diesel::table! {
    contents (version_id) {
        version_id -> BigInt,
        data -> Binary,
    }
}

diesel::table! {
    file_paths (id) {
        id -> BigInt,
        path -> Text,
        created_at -> BigInt,
        last_modified -> BigInt,
    }
}

diesel::table! {
    versions (id) {
        id -> BigInt,
        file_path_id -> BigInt,
        operation_type -> Text,
        timestamp -> BigInt,
        size -> BigInt,
        content_hash -> Nullable<Text>,
        description -> Nullable<Text>,
    }
}

diesel::table! {
    versions_fts (id) {
        id -> BigInt,
        description -> Text,
    }
}

diesel::joinable!(contents -> versions (version_id));
diesel::joinable!(versions -> file_paths (file_path_id));

diesel::allow_tables_to_appear_in_same_query!(contents, file_paths, versions, versions_fts,);
