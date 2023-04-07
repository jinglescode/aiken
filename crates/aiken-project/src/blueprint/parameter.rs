use super::{
    definitions::{Definitions, Reference},
    error::Error,
    schema::{Annotated, Constructor, Data, Declaration, Items, Schema},
};
use std::{iter, ops::Deref, rc::Rc};
use uplc::{
    ast::{Constant, Data as UplcData, DeBruijn, Term},
    PlutusData,
};

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub struct Parameter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    pub schema: Reference,
}

type Instance = Term<DeBruijn>;

impl From<Reference> for Parameter {
    fn from(schema: Reference) -> Parameter {
        Parameter {
            title: None,
            schema,
        }
    }
}

impl Parameter {
    pub fn validate(
        &self,
        definitions: &Definitions<Annotated<Schema>>,
        term: &Instance,
    ) -> Result<(), Error> {
        let schema = &definitions
            .lookup(&self.schema)
            .map(Ok)
            .unwrap_or_else(|| {
                Err(Error::UnresolvedSchemaReference {
                    reference: self.schema.clone(),
                })
            })?
            .annotated;

        validate_schema(schema, definitions, term)
    }
}

fn validate_schema(
    schema: &Schema,
    definitions: &Definitions<Annotated<Schema>>,
    term: &Instance,
) -> Result<(), Error> {
    match schema {
        Schema::Data(data) => validate_data(data, definitions, term),

        Schema::Unit => expect_unit(term),

        Schema::Integer => expect_integer(term),

        Schema::Bytes => expect_bytes(term),

        Schema::String => expect_string(term),

        Schema::Boolean => expect_boolean(term),

        Schema::Pair(left, right) => {
            let (term_left, term_right) = expect_pair(term)?;

            let left =
                left.schema(definitions)
                    .ok_or_else(|| Error::UnresolvedSchemaReference {
                        reference: left.reference().unwrap().clone(),
                    })?;
            validate_schema(left, definitions, &term_left)?;

            let right =
                right
                    .schema(definitions)
                    .ok_or_else(|| Error::UnresolvedSchemaReference {
                        reference: right.reference().unwrap().clone(),
                    })?;
            validate_schema(right, definitions, &term_right)?;

            Ok(())
        }

        Schema::List(Items::One(item)) => {
            let terms = expect_list(term)?;

            let item =
                item.schema(definitions)
                    .ok_or_else(|| Error::UnresolvedSchemaReference {
                        reference: item.reference().unwrap().clone(),
                    })?;

            for ref term in terms {
                validate_schema(item, definitions, term)?;
            }

            Ok(())
        }

        Schema::List(Items::Many(items)) => {
            let terms = expect_list(term)?;

            let items = items
                .iter()
                .map(|item| {
                    item.schema(definitions)
                        .ok_or_else(|| Error::UnresolvedSchemaReference {
                            reference: item.reference().unwrap().clone(),
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;

            if terms.len() != items.len() {
                return Err(Error::TupleItemsMismatch {
                    expected: items.len(),
                    found: terms.len(),
                });
            }

            for (item, ref term) in iter::zip(items, terms) {
                validate_schema(item, definitions, term)?;
            }

            Ok(())
        }
    }
}

fn validate_data(
    data: &Data,
    definitions: &Definitions<Annotated<Schema>>,
    term: &Instance,
) -> Result<(), Error> {
    match data {
        Data::Opaque => expect_data(term),

        Data::Integer => expect_data_integer(term),

        Data::Bytes => expect_data_bytes(term),

        Data::List(Items::One(item)) => {
            let terms = expect_data_list(term)?;

            let item =
                item.schema(definitions)
                    .ok_or_else(|| Error::UnresolvedSchemaReference {
                        reference: item.reference().unwrap().clone(),
                    })?;

            for ref term in terms {
                validate_data(item, definitions, term)?;
            }

            Ok(())
        }

        Data::List(Items::Many(items)) => {
            let terms = expect_data_list(term)?;

            let items = items
                .iter()
                .map(|item| {
                    item.schema(definitions)
                        .ok_or_else(|| Error::UnresolvedSchemaReference {
                            reference: item.reference().unwrap().clone(),
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;

            if terms.len() != items.len() {
                return Err(Error::TupleItemsMismatch {
                    expected: items.len(),
                    found: terms.len(),
                });
            }

            for (item, ref term) in iter::zip(items, terms) {
                validate_data(item, definitions, term)?;
            }

            Ok(())
        }

        Data::Map(keys, values) => {
            let terms = expect_data_map(term)?;

            let keys =
                keys.schema(definitions)
                    .ok_or_else(|| Error::UnresolvedSchemaReference {
                        reference: keys.reference().unwrap().clone(),
                    })?;

            let values =
                values
                    .schema(definitions)
                    .ok_or_else(|| Error::UnresolvedSchemaReference {
                        reference: values.reference().unwrap().clone(),
                    })?;

            for (ref k, ref v) in terms {
                validate_data(keys, definitions, k)?;
                validate_data(values, definitions, v)?;
            }

            Ok(())
        }

        Data::AnyOf(constructors) => {
            let constructors: Vec<(usize, Vec<&Data>)> = constructors
                .iter()
                .map(|constructor| {
                    constructor
                        .annotated
                        .fields
                        .iter()
                        .map(|field| {
                            field.annotated.schema(definitions).ok_or_else(|| {
                                Error::UnresolvedSchemaReference {
                                    reference: field.annotated.reference().unwrap().clone(),
                                }
                            })
                        })
                        .collect::<Result<_, _>>()
                        .map(|fields| (constructor.annotated.index, fields))
                })
                .collect::<Result<_, _>>()?;

            for (index, fields_schema) in constructors.iter() {
                if let Ok(fields) = expect_data_constr(term, *index) {
                    if fields_schema.len() != fields.len() {
                        panic!("fields length different");
                    }

                    for (instance, schema) in iter::zip(fields, fields_schema) {
                        validate_data(schema, definitions, &instance)?;
                    }

                    return Ok(());
                }
            }

            Err(Error::SchemaMismatch {
                schema: Schema::Data(Data::AnyOf(
                    constructors
                        .iter()
                        .map(|(index, fields)| {
                            Constructor {
                                index: *index,
                                fields: fields
                                    .iter()
                                    .map(|_| Declaration::Inline(Box::new(Data::Opaque)).into())
                                    .collect(),
                            }
                            .into()
                        })
                        .collect(),
                )),
                term: term.clone(),
            })
        }
    }
}

fn expect_data(term: &Instance) -> Result<(), Error> {
    if let Term::Constant(constant) = term {
        if matches!(constant.deref(), Constant::Data(..)) {
            return Ok(());
        }
    }

    Err(Error::SchemaMismatch {
        schema: Schema::Data(Data::Opaque),
        term: term.clone(),
    })
}

fn expect_data_integer(term: &Instance) -> Result<(), Error> {
    if let Term::Constant(constant) = term {
        if let Constant::Data(data) = constant.deref() {
            if matches!(data, PlutusData::BigInt(..)) {
                return Ok(());
            }
        }
    }

    Err(Error::SchemaMismatch {
        schema: Schema::Data(Data::Integer),
        term: term.clone(),
    })
}

fn expect_data_bytes(term: &Instance) -> Result<(), Error> {
    if let Term::Constant(constant) = term {
        if let Constant::Data(data) = constant.deref() {
            if matches!(data, PlutusData::BoundedBytes(..)) {
                return Ok(());
            }
        }
    }

    Err(Error::SchemaMismatch {
        schema: Schema::Data(Data::Bytes),
        term: term.clone(),
    })
}

fn expect_data_list(term: &Instance) -> Result<Vec<Instance>, Error> {
    if let Term::Constant(constant) = term {
        if let Constant::Data(PlutusData::Array(elems)) = constant.deref() {
            return Ok(elems
                .iter()
                .map(|elem| Term::Constant(Rc::new(Constant::Data(elem.to_owned()))))
                .collect());
        }
    }

    let inner_schema = Items::One(Declaration::Inline(Box::new(Data::Opaque)));

    Err(Error::SchemaMismatch {
        schema: Schema::Data(Data::List(inner_schema)),
        term: term.clone(),
    })
}

fn expect_data_map(term: &Instance) -> Result<Vec<(Instance, Instance)>, Error> {
    if let Term::Constant(constant) = term {
        if let Constant::Data(PlutusData::Map(pairs)) = constant.deref() {
            return Ok(pairs
                .iter()
                .map(|(k, v)| {
                    (
                        Term::Constant(Rc::new(Constant::Data(k.to_owned()))),
                        Term::Constant(Rc::new(Constant::Data(v.to_owned()))),
                    )
                })
                .collect());
        }
    }

    let key_schema = Declaration::Inline(Box::new(Data::Opaque));
    let value_schema = Declaration::Inline(Box::new(Data::Opaque));

    Err(Error::SchemaMismatch {
        schema: Schema::Data(Data::Map(key_schema, value_schema)),
        term: term.clone(),
    })
}

fn expect_data_constr(term: &Instance, index: usize) -> Result<Vec<Instance>, Error> {
    if let Term::Constant(constant) = term {
        if let Constant::Data(PlutusData::Constr(constr)) = constant.deref() {
            if let PlutusData::Constr(expected) = UplcData::constr(index as u64, vec![]) {
                if expected.tag == constr.tag && expected.any_constructor == constr.any_constructor
                {
                    return Ok(constr
                        .fields
                        .iter()
                        .map(|field| Term::Constant(Rc::new(Constant::Data(field.to_owned()))))
                        .collect());
                }
            }
        }
    }

    Err(Error::SchemaMismatch {
        schema: Schema::Data(Data::AnyOf(vec![Constructor {
            index,
            fields: vec![],
        }
        .into()])),
        term: term.clone(),
    })
}

fn expect_unit(term: &Instance) -> Result<(), Error> {
    if let Term::Constant(constant) = term {
        if matches!(constant.deref(), Constant::Unit) {
            return Ok(());
        }
    }

    Err(Error::SchemaMismatch {
        schema: Schema::Unit,
        term: term.clone(),
    })
}

fn expect_integer(term: &Instance) -> Result<(), Error> {
    if let Term::Constant(constant) = term {
        if matches!(constant.deref(), Constant::Integer(..)) {
            return Ok(());
        }
    }

    Err(Error::SchemaMismatch {
        schema: Schema::Integer,
        term: term.clone(),
    })
}

fn expect_bytes(term: &Instance) -> Result<(), Error> {
    if let Term::Constant(constant) = term {
        if matches!(constant.deref(), Constant::ByteString(..)) {
            return Ok(());
        }
    }

    Err(Error::SchemaMismatch {
        schema: Schema::Bytes,
        term: term.clone(),
    })
}

fn expect_string(term: &Instance) -> Result<(), Error> {
    if let Term::Constant(constant) = term {
        if matches!(constant.deref(), Constant::String(..)) {
            return Ok(());
        }
    }

    Err(Error::SchemaMismatch {
        schema: Schema::String,
        term: term.clone(),
    })
}

fn expect_boolean(term: &Instance) -> Result<(), Error> {
    if let Term::Constant(constant) = term {
        if matches!(constant.deref(), Constant::Bool(..)) {
            return Ok(());
        }
    }

    Err(Error::SchemaMismatch {
        schema: Schema::Boolean,
        term: term.clone(),
    })
}

fn expect_pair(term: &Instance) -> Result<(Instance, Instance), Error> {
    if let Term::Constant(constant) = term {
        if let Constant::ProtoPair(_, _, left, right) = constant.deref() {
            return Ok((Term::Constant(left.clone()), Term::Constant(right.clone())));
        }
    }

    let left_schema = Declaration::Inline(Box::new(Schema::Data(Data::Opaque)));
    let right_schema = Declaration::Inline(Box::new(Schema::Data(Data::Opaque)));

    Err(Error::SchemaMismatch {
        schema: Schema::Pair(left_schema, right_schema),
        term: term.clone(),
    })
}

fn expect_list(term: &Instance) -> Result<Vec<Instance>, Error> {
    if let Term::Constant(constant) = term {
        if let Constant::ProtoList(_, elems) = constant.deref() {
            return Ok(elems
                .iter()
                .map(|elem| Term::Constant(Rc::new(elem.to_owned())))
                .collect());
        }
    }

    let inner_schema = Items::One(Declaration::Inline(Box::new(Schema::Data(Data::Opaque))));

    Err(Error::SchemaMismatch {
        schema: Schema::List(inner_schema),
        term: term.clone(),
    })
}
