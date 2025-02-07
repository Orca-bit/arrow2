use std::sync::Arc;

use arrow2::{
    array::*,
    datatypes::{DataType, Field},
};

#[test]
fn basics() {
    let dt = DataType::Struct(vec![
        Field::new("a", DataType::Utf8, true),
        Field::new("b", DataType::Utf8, true),
    ]);
    let data_type = DataType::Map(Box::new(Field::new("a", dt.clone(), true)), false);

    let field = StructArray::new(
        dt.clone(),
        vec![
            Arc::new(Utf8Array::<i32>::from_slice(["a", "aa", "aaa"])) as _,
            Arc::new(Utf8Array::<i32>::from_slice(["b", "bb", "bbb"])),
        ],
        None,
    );

    let array = MapArray::new(data_type, vec![0, 1, 2].into(), Arc::new(field), None);

    assert_eq!(
        array.value(0),
        Box::new(StructArray::new(
            dt.clone(),
            vec![
                Arc::new(Utf8Array::<i32>::from_slice(["a"])) as _,
                Arc::new(Utf8Array::<i32>::from_slice(["b"])),
            ],
            None,
        )) as Box<dyn Array>
    );

    let sliced = array.slice(1, 1);
    assert_eq!(
        sliced.value(0),
        Box::new(StructArray::new(
            dt,
            vec![
                Arc::new(Utf8Array::<i32>::from_slice(["aa"])) as _,
                Arc::new(Utf8Array::<i32>::from_slice(["bb"])),
            ],
            None,
        )) as Box<dyn Array>
    );
}
