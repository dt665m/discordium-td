/// Declare an exact bounded schema without runtime reflection. Field IDs are
/// literals and determine wire order; declaration order and Rust names do not.
/// Defaults are trusted initialization expressions and must be deterministic.
///
/// ```
/// use engine_net::{schema, schema::*, types::SchemaId};
/// schema! {
///     pub struct PublicCounter (SchemaId(9), 1, Representation::PublicPresentation) {
///         1 => count: u64 = 0 => FieldSpec::exact(
///             Units::Unitless, Range::Unsigned { minimum: 0, maximum: u64::MAX },
///             Visibility::Public, Prediction::None),
///     }
/// }
/// ```
#[macro_export]
macro_rules! schema {
    ($(#[$meta:meta])* $vis:vis struct $name:ident ($schema:expr,$version:expr,$representation:expr) {
        $($id:literal => $field:ident : $ty:ty = $default:expr => $spec:expr),+ $(,)?
    }) => {
        $(#[$meta])*
        #[derive(Clone,Debug,PartialEq)]
        $vis struct $name {$(pub $field:$ty),+}
        impl $crate::schema::Schema for $name {
            const ID:$crate::types::SchemaId=$schema;
            const VERSION:u32=$version;
            const REPRESENTATION:$crate::schema::Representation=$representation;
            const FIELDS:&'static[$crate::schema::FieldDescriptor]=&[$($crate::schema::FieldDescriptor{id:$crate::schema::FieldId($id),name:stringify!($field),spec:$spec}),+];
            fn defaults()->Self {Self{$($field:$default),+}}
            fn inspect_fields(i:&mut $crate::schema::Inspection,depth:usize,visibility:$crate::schema::Visibility,prediction:$crate::schema::Prediction)->Result<(),$crate::schema::SchemaError>{
                let (ids,count)=$crate::schema::ordered(Self::FIELDS,i.limits.fields)?;
                for id in &ids[..count] {
                    #[allow(unreachable_patterns)]
                    match id.0 {$($id=>i.field::<$ty>(&$crate::schema::FieldDescriptor{id:*id,name:stringify!($field),spec:$spec},depth,visibility,prediction,Self::VERSION)?,)+_=>unreachable!()}
                }
                Ok(())
            }
            fn maximum_fields(work:&mut $crate::schema::Work,depth:usize)->Result<usize,$crate::schema::SchemaError>{
                let mut size=0usize;
                $(let field_size=<$ty as $crate::schema::FieldValue>::maximum_size(work,depth+1)?;size=work.size(size+6+field_size)?;)+
                Ok(size)
            }
            fn fields_size(&self,work:&mut $crate::schema::Work,depth:usize)->Result<usize,$crate::schema::SchemaError>{
                let mut size=0usize;
                $(let field_size=<$ty as $crate::schema::FieldValue>::value_size(&self.$field,&$spec,work,depth+1)?;size=work.size(size+6+field_size)?;)+
                Ok(size)
            }
            fn write_fields(&self,out:&mut Vec<u8>,work:&mut $crate::schema::Work,depth:usize)->Result<(),$crate::schema::SchemaError>{
                let (ids,count)=$crate::schema::ordered(Self::FIELDS,work.limits.fields)?;
                for id in &ids[..count] {
                    #[allow(unreachable_patterns)]
                    match id.0 {$($id=>$crate::schema::write_field(*id,&self.$field,out,work,depth)?,)+_=>unreachable!()}
                }
                Ok(())
            }
            fn read_fields(reader:&mut $crate::schema::Reader<'_>,work:&mut $crate::schema::Work,depth:usize,count:usize)->Result<Self,$crate::schema::SchemaError>{
                $(let mut $field:Option<$ty>=None;)+
                for _ in 0..count {
                    let (id,bytes)=reader.field()?;
                    #[allow(unreachable_patterns)]
                    match id.0 {$($id=>{$field=Some(<$ty as $crate::schema::FieldValue>::read(bytes,work,depth+1)?);},)+_=>return Err($crate::schema::SchemaError::UnknownField(id))}
                }
                Ok(Self{$($field:$field.ok_or($crate::schema::SchemaError::MissingField($crate::schema::FieldId($id)))?),+})
            }
            fn diagnose(before:&Self,after:&Self,access:&$crate::schema::DiagnosticAccess<'_>,out:&mut Vec<$crate::schema::FieldChange>,work:&mut $crate::schema::Work)->Result<(),$crate::schema::SchemaError>{
                let (ids,count)=$crate::schema::ordered(Self::FIELDS,work.limits.fields)?;
                for id in &ids[..count] {
                    #[allow(unreachable_patterns)]
                    match id.0 {$($id=>$crate::schema::diagnostic_field(&$crate::schema::FieldDescriptor{id:*id,name:stringify!($field),spec:$spec},&before.$field,&after.$field,access,out,work)?,)+_=>unreachable!()}
                }
                Ok(())
            }
        }
        impl $crate::schema::FieldValue for $name {
            fn inspect(spec:&$crate::schema::FieldSpec,i:&mut $crate::schema::Inspection,depth:usize)->Result<(),$crate::schema::SchemaError>{
                if spec.range!=$crate::schema::Range::None{return Err($crate::schema::SchemaError::InvalidRange);}
                if !matches!(spec.interpolation,$crate::schema::Interpolation::None|$crate::schema::Interpolation::Step){return Err($crate::schema::SchemaError::InvalidInterpolation);}
                i.schema::<Self>(depth,spec.visibility,spec.prediction)
            }
            fn maximum_size(work:&mut $crate::schema::Work,depth:usize)->Result<usize,$crate::schema::SchemaError>{$crate::schema::record_maximum::<Self>(work,depth)}
            fn value_size(&self,spec:&$crate::schema::FieldSpec,work:&mut $crate::schema::Work,depth:usize)->Result<usize,$crate::schema::SchemaError>{
                if spec.range!=$crate::schema::Range::None{return Err($crate::schema::SchemaError::InvalidRange);}
                $crate::schema::record_size(self,work,depth)
            }
            fn write(&self,out:&mut Vec<u8>,work:&mut $crate::schema::Work,depth:usize)->Result<(),$crate::schema::SchemaError>{$crate::schema::write_record(self,out,work,depth)}
            fn read(bytes:&[u8],work:&mut $crate::schema::Work,depth:usize)->Result<Self,$crate::schema::SchemaError>{$crate::schema::read_record::<Self>(bytes,work,depth)}
        }
        impl $crate::schema::CompactValue for $name {
            fn write_compact(&self,out:&mut Vec<u8>,work:&mut $crate::schema::Work,depth:usize)->Result<(),$crate::schema::SchemaError>{
                work.node(depth)?;
                let (ids,count)=$crate::schema::ordered(<Self as $crate::schema::Schema>::FIELDS,work.limits.fields)?;
                for id in &ids[..count] {
                    #[allow(unreachable_patterns)]
                    match id.0 {$($id=><$ty as $crate::schema::CompactValue>::write_compact(&self.$field,out,work,depth+1)?,)+_=>unreachable!()}
                }
                Ok(())
            }
            fn read_compact(reader:&mut $crate::schema::Reader<'_>,work:&mut $crate::schema::Work,depth:usize)->Result<Self,$crate::schema::SchemaError>{
                work.node(depth)?;
                let (ids,count)=$crate::schema::ordered(<Self as $crate::schema::Schema>::FIELDS,work.limits.fields)?;
                $(let mut $field:Option<$ty>=None;)+
                for id in &ids[..count] {
                    #[allow(unreachable_patterns)]
                    match id.0 {$($id=>{$field=Some(<$ty as $crate::schema::CompactValue>::read_compact(reader,work,depth+1)?);},)+_=>unreachable!()}
                }
                Ok(Self{$($field:$field.ok_or($crate::schema::SchemaError::MissingField($crate::schema::FieldId($id)))?),+})
            }
        }
    };
}

/// Declare explicit stable u16 enum tags. Unit variants and single typed-payload
/// variants are supported; unknown tags reject before a payload is allocated.
/// Names and declaration order are diagnostic only. Payloads use bounded
/// FieldValue implementations, including nested schemas with their own policies.
#[macro_export]
macro_rules! schema_enum {
    ($vis:vis enum $name:ident { $($id:literal => $variant:ident),+ $(,)? }) => {
        #[derive(Clone,Copy,Debug,PartialEq,Eq)]
        $vis enum $name { $($variant),+ }
        impl $crate::schema::FieldValue for $name {
            fn inspect(s:&$crate::schema::FieldSpec,i:&mut $crate::schema::Inspection,_:usize)->Result<(),$crate::schema::SchemaError> {
                if s.range!=$crate::schema::Range::None { return Err($crate::schema::SchemaError::InvalidRange); }
                if !matches!(s.interpolation,$crate::schema::Interpolation::None|$crate::schema::Interpolation::Step) { return Err($crate::schema::SchemaError::InvalidInterpolation); }
                let (ids,count)=$crate::schema::enum_ids(&[$($id),+])?;i.tag(16);i.number(count as u64);for id in &ids[..count] {i.number(u64::from(*id));}Ok(())
            }
            fn maximum_size(w:&mut $crate::schema::Work,d:usize)->Result<usize,$crate::schema::SchemaError> {w.node(d)?;Ok(2)}
            fn value_size(&self,_:&$crate::schema::FieldSpec,w:&mut $crate::schema::Work,d:usize)->Result<usize,$crate::schema::SchemaError> {w.node(d)?;Ok(2)}
            fn write(&self,out:&mut Vec<u8>,w:&mut $crate::schema::Work,d:usize)->Result<(),$crate::schema::SchemaError> {
                w.node(d)?;let tag:u16=match self {$(Self::$variant=>$id),+};w.append(out,&tag.to_le_bytes())
            }
            fn read(bytes:&[u8],w:&mut $crate::schema::Work,d:usize)->Result<Self,$crate::schema::SchemaError> {
                w.node(d)?;let tag=u16::from_le_bytes(bytes.try_into().map_err(|_|$crate::schema::SchemaError::InvalidEncoding)?);
                #[allow(unreachable_patterns)]
                match tag {$($id=>Ok(Self::$variant),)+_=>Err($crate::schema::SchemaError::InvalidEncoding)}
            }
        }
        impl $crate::schema::CompactValue for $name {
            fn write_compact(&self,out:&mut Vec<u8>,work:&mut $crate::schema::Work,depth:usize)->Result<(),$crate::schema::SchemaError>{
                <Self as $crate::schema::FieldValue>::write(self,out,work,depth)
            }
            fn read_compact(reader:&mut $crate::schema::Reader<'_>,work:&mut $crate::schema::Work,depth:usize)->Result<Self,$crate::schema::SchemaError>{
                <Self as $crate::schema::FieldValue>::read(reader.take(2)?,work,depth)
            }
        }
    };
    ($vis:vis enum $name:ident { $($id:literal => $variant:ident ($ty:ty)),+ $(,)? }) => {
        #[derive(Clone,Debug,PartialEq)]
        $vis enum $name { $($variant($ty)),+ }
        impl $crate::schema::FieldValue for $name {
            fn inspect(s:&$crate::schema::FieldSpec,i:&mut $crate::schema::Inspection,d:usize)->Result<(),$crate::schema::SchemaError> {
                if s.range!=$crate::schema::Range::None { return Err($crate::schema::SchemaError::InvalidRange); }
                if !matches!(s.interpolation,$crate::schema::Interpolation::None|$crate::schema::Interpolation::Step) { return Err($crate::schema::SchemaError::InvalidInterpolation); }
                let (ids,count)=$crate::schema::enum_ids(&[$($id),+])?;i.tag(17);i.number(count as u64);
                for id in &ids[..count] {i.number(u64::from(*id));#[allow(unreachable_patterns)] match *id {$($id=><$ty as $crate::schema::FieldValue>::inspect(s,i,d+1)?,)+_=>unreachable!()}}Ok(())
            }
            fn maximum_size(w:&mut $crate::schema::Work,d:usize)->Result<usize,$crate::schema::SchemaError> {
                w.node(d)?;let before=w.clone();let mut size=0;
                $(let mut branch=before.clone();size=size.max(<$ty as $crate::schema::FieldValue>::maximum_size(&mut branch,d+1)?);w.merge_max(&branch);)+w.size(2+size)
            }
            fn value_size(&self,s:&$crate::schema::FieldSpec,w:&mut $crate::schema::Work,d:usize)->Result<usize,$crate::schema::SchemaError> {w.node(d)?;let size=match self {$(Self::$variant(value)=><$ty as $crate::schema::FieldValue>::value_size(value,s,w,d+1)?),+};w.size(2+size)}
            fn write(&self,out:&mut Vec<u8>,w:&mut $crate::schema::Work,d:usize)->Result<(),$crate::schema::SchemaError> {
                w.node(d)?;match self {$(Self::$variant(value)=>{w.append(out,&($id as u16).to_le_bytes())?;<$ty as $crate::schema::FieldValue>::write(value,out,w,d+1)}),+}
            }
            fn read(bytes:&[u8],w:&mut $crate::schema::Work,d:usize)->Result<Self,$crate::schema::SchemaError> {
                w.node(d)?;let mut reader=$crate::schema::Reader::new(bytes);let tag=reader.u16()?;let body=reader.take(reader.remaining())?;
                #[allow(unreachable_patterns)] match tag {$($id=><$ty as $crate::schema::FieldValue>::read(body,w,d+1).map(Self::$variant),)+_=>Err($crate::schema::SchemaError::InvalidEncoding)}
            }
        }
        impl $crate::schema::CompactValue for $name {
            fn write_compact(&self,out:&mut Vec<u8>,work:&mut $crate::schema::Work,depth:usize)->Result<(),$crate::schema::SchemaError>{
                work.node(depth)?;
                match self {$(Self::$variant(value)=>{work.append(out,&($id as u16).to_le_bytes())?;<$ty as $crate::schema::CompactValue>::write_compact(value,out,work,depth+1)}),+}
            }
            fn read_compact(reader:&mut $crate::schema::Reader<'_>,work:&mut $crate::schema::Work,depth:usize)->Result<Self,$crate::schema::SchemaError>{
                work.node(depth)?;
                let tag=reader.u16()?;
                #[allow(unreachable_patterns)]
                match tag {$($id=><$ty as $crate::schema::CompactValue>::read_compact(reader,work,depth+1).map(Self::$variant),)+_=>Err($crate::schema::SchemaError::InvalidEncoding)}
            }
        }
    };
}
