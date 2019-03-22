use std::collections::HashMap;
use core::borrow::Borrow;
use crate::lib::RustExpressionEngine::node::{Node, NullNode, OptNode, BoolNode, StringNode, NumberNode, ArgNode, BinaryNode, NodeItem};
use crate::lib::RustExpressionEngine::node::NodeType::NOpt;
use crate::lib::RustExpressionEngine::runtime::{IsNumber, OptMap, ParserTokens};
use std::collections::linked_list::LinkedList;


//TODO 解决bug

pub fn Parser(express: String, optMap: &OptMap) -> (NodeItem, String) {
    let tokens = ParserTokens(&express);

    let mut nodes = vec![];
    for item in tokens {
        let node = NodeItem::New(item);
//        if err != "" {
//            return (NodeItem::New("".to_string()), err);
//        }
        nodes.push(node);
    }
//    //TODO check nodes
//
//
    for item in optMap.priorityArray() {
          findReplaceOpt(optMap,&express,&item,&mut nodes);
    }
//
    for item in nodes{
        println!("{}:{}",item.Type(),item.Value());
    }


    return (NodeItem::New("".to_string()), "".to_string());
}

fn findReplaceOpt(optMap:&OptMap,express: &String, operator: &str, nodeArg: &mut Vec<NodeItem>) -> String {

    //let nodes=vec![];
    let mut index = 0 as i32;
    let nodeArgLen=nodeArg.len();
    for item in nodeArg.clone(){
        let itemType = item.Type();
        if itemType as i32 == NOpt as i32 && operator==item.Value().as_str().unwrap(){
            let leftIndex = (index - 1) as usize;
            let rightIndex = (index + 1) as usize;
            let left=nodeArg[leftIndex].clone();
            let right=nodeArg[rightIndex].clone();
            let binaryNode=BinaryNode::NewItem(left, right, item.Value().as_str().unwrap().to_string());
            println!("binNode={}",&binaryNode.Type());

            let binaryNodeItem= NodeItem::NewNBinaryNode(binaryNode);

            nodeArg.remove(rightIndex-1);
            nodeArg.remove(index as usize);
            nodeArg.remove(leftIndex);

            nodeArg.insert(leftIndex,binaryNodeItem);
            if haveOpt(nodeArg){
                findReplaceOpt(optMap,express,operator,nodeArg);
            }
            break;
        }
        index = index + 1;
    }
    return "".to_string();
}

fn haveOpt(nodeArg: &mut Vec<NodeItem>)->bool{
    for item in nodeArg{
        if item.Type() as i32==NOpt as i32{
            return true;
        }
    }
    return false;
}